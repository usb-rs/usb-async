//! Future-based USB interface.
#![deny(missing_docs)]
#![allow(clippy::cast_possible_truncation)]

use std::{
    cell::RefCell,
    convert::TryFrom,
    error::Error as StdError,
    fmt,
    io,
};

use tokio::prelude::*;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod os;

#[cfg(not(target_os = "linux"))]
compile_error!("Sorry, usb-async has not been ported to your platform yet.");

/// USB errors.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Error {
    /// An invalid ID was passed as argument.
    InvalidId,
    /// The device the ID refers to is not connected.
    NotConnected,
    /// An io::Error occurred.
    Io(io::ErrorKind),
}

impl From<os::UsbError> for Error {
    fn from(err: os::UsbError) -> Self {
        match err {
            os::UsbError::InvalidId => Error::InvalidId,
            os::UsbError::NotConnected => Error::NotConnected,
            os::UsbError::Io(io) => Error::Io(io),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::InvalidId => write!(f, "an invalid device was specified"),
            Error::NotConnected => write!(f, "the specified device is not connected"),
            Error::Io(io) => write!(f, "an io error occurred: {:?}", io),
        }
    }
}

impl StdError for Error {}

/// A handle to a USB device.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct Id(os::Id);

impl From<Id> for os::Id {
    fn from(id: Id) -> Self {
        id.0
    }
}

impl From<os::Id> for Id {
    fn from(id: os::Id) -> Self {
        Self(id)
    }
}

/// A USB hotplug event.
#[derive(Copy, Clone, Debug, PartialEq, Hash)]
pub enum Event {
    /// A USB device was plugged in.
    Add(Id),
    /// A USB device was removed.
    Remove(Id),
}

impl TryFrom<os::Event> for Event {
    type Error = ();

    fn try_from(event: os::Event) -> Result<Self, ()> {
        match event {
            os::Event::Add(id) => Ok(Event::Add(id.into())),
            os::Event::Remove(id) => Ok(Event::Remove(id.into())),
            os::Event::Change(_) | os::Event::Unknown => Err(()),
        }
    }
}

/// A USB hotplug event monitor.
pub struct HotplugMonitor<'a> {
    monitor: os::Monitor<'a>,
    context: &'a Context,
}

impl Stream for HotplugMonitor<'_> {
    type Item = Event;
    type Error = Error;

    fn poll(&mut self) -> Result<Async<Option<Event>>, Error> {
        match self.monitor.poll() {
            Ok(Async::Ready(Some(ev))) => {
                match Event::try_from(ev) {
                    Ok(ev) => {
                        match ev {
                            Event::Add(id) => {
                                self.context.add(id);
                                Ok(Async::Ready(Some(Event::Add(id))))
                            },
                            Event::Remove(id) => {
                                Ok(Async::Ready(Some(Event::Remove(id))))
                            },
                        }
                    },
                    // Drop messages we don't understand.
                    Err(()) => Ok(Async::NotReady),
                }
            }
            Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(err) => Err(err.into()),
        }
    }
}

struct Metadata {
    vendor_id: Option<u16>,
    product_id: Option<u16>,
}

/// A USB context.
pub struct Context {
    context: os::Context,
    metadata: RefCell<Vec<Metadata>>,
}

impl Context {
    fn add(&self, id: Id) {
        let vendor_id = self.context.vendor_id(id.into()).ok();
        let product_id = self.context.product_id(id.into()).ok();
        let metadata = Metadata {
            vendor_id,
            product_id,
        };
        self.metadata.borrow_mut().push(metadata);
    }

    /// Create a USB context.
    pub fn new() -> Result<Self, Box<dyn StdError>> {
        let context = Self {
            context: os::Context::new()?,
            metadata: RefCell::new(Vec::new()),
        };

        for dev in context.devices() {
            context.add(dev);
        }

        Ok(context)
    }

    /// Create a USB hotplug monitor.
    pub fn monitor(&self) -> Result<HotplugMonitor<'_>, Box<dyn StdError>> {
        Ok(HotplugMonitor {
            monitor: self.context.monitor()?,
            context: self,
        })
    }

    /// Is a device plugged in?
    pub fn is_connected(&self, id: Id) -> bool {
        self.context.is_connected(id.into())
    }

    /// Retrieve the USB vendor ID of a device.
    pub fn vendor_id(&self, id: Id) -> Option<u16> {
        self.metadata.borrow()[(id.0).0 as usize].vendor_id
    }

    /// Retrieve the USB product ID of a device.
    pub fn product_id(&self, id: Id) -> Option<u16> {
        self.metadata.borrow()[(id.0).0 as usize].product_id
    }

    /// Retrieve the USB manufacturer string of a device.
    pub fn manufacturer_string(&self, id: Id) -> Result<String, Error> {
        self.context
            .manufacturer_string(id.into())
            .map_err(std::convert::Into::into)
    }

    /// Retrieve the USB product string of a device.
    pub fn product_string(&self, id: Id) -> Result<String, Error> {
        self.context
            .product_string(id.into())
            .map_err(std::convert::Into::into)
    }

    /// Iterate through all devices, both connected and disconnected.
    ///
    /// Use `connected_devices` to only iterate over currently plugged in devices.
    pub fn devices(&self) -> impl Iterator<Item = Id> {
        self.context.devices().map(Id)
    }

    /// Iterate through connected devices.
    pub fn connected_devices(&self) -> impl Iterator<Item = Id> + '_ {
        self.devices().filter(move |id| self.is_connected(*id))
    }
}
