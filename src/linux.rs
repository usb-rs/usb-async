use std::{
    cell::RefCell,
    error, io,
    os::unix::io::AsRawFd,
    path::{Path, PathBuf},
};

use udev;
use mio;
use tokio::{prelude::*, reactor};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct Id(pub u32);

impl From<Id> for usize {
    fn from(id: Id) -> Self {
        let Id(id) = id;
        id as Self
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    Add(Id),
    Remove(Id),
    Change(Id),
    Unknown,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UsbError {
    InvalidId,
    NotConnected,
    Io(io::ErrorKind),
}

impl From<udev::Error> for UsbError {
    fn from(err: udev::Error) -> Self {
        use udev::ErrorKind;
        match err.kind() {
            ErrorKind::NoMem => panic!("usb-async: error from udev: out of memory"),
            ErrorKind::InvalidInput => panic!("usb-async: error from udev: invalid input\nThis is probably a usb-async bug; please report it"),
            ErrorKind::Io(io_err) => UsbError::Io(io_err),
        }
    }
}

impl From<io::Error> for UsbError {
    fn from(err: io::Error) -> Self {
        UsbError::Io(err.kind())
    }
}

pub struct Monitor<'a> {
    context: &'a Context,
    socket: udev::MonitorSocket,
    reg: reactor::Registration,
}

impl Stream for Monitor<'_> {
    type Item = Event;
    type Error = UsbError; // Can this actually fail?

    fn poll(&mut self) -> Result<Async<Option<Event>>, UsbError> {
        self.reg
            .register(&mio::unix::EventedFd(&self.socket.as_raw_fd()))?;

        match self.reg.poll_read_ready()? {
            Async::Ready(readiness) => {
                if readiness.is_readable() {
                    if let Some(event) = self.socket.next() {
                        let device = event.device();
                        let path = device.syspath();
                        println!("Got {} event on {}", event.device().property_value("ACTION").unwrap().to_str().unwrap(), path.display());

                        match event.event_type() {
                            udev::EventType::Add => {
                                match self.context.add_device(path) {
                                    Some(id) => Ok(Async::Ready(Some(Event::Add(id)))),
                                    None => Ok(Async::NotReady),
                                }
                            },
                            udev::EventType::Remove => {
                                match self.context.remove_device_by_path(path) {
                                    Some(id) => Ok(Async::Ready(Some(Event::Remove(id)))),
                                    None => Ok(Async::NotReady),
                                }
                            },
                            udev::EventType::Change | udev::EventType::Unknown => {
                                Ok(Async::NotReady) // For now
                            }
                        }
                    } else {
                        Ok(Async::NotReady)
                    }
                } else {
                    Ok(Async::NotReady)
                }
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub struct Context {
    udev: udev::Context,
    paths: RefCell<Vec<Option<PathBuf>>>,
}

impl Context {
    pub fn new() -> Result<Self, Box<dyn error::Error>> {
        let context = Self {
            udev: udev::Context::new()?,
            paths: RefCell::new(Vec::new()),
        };

        {
            // Scan for currently connected devices.
            let mut enumerator = udev::Enumerator::new(&context.udev)?;
            enumerator.match_subsystem("usb")?;
            for dev in enumerator.scan_devices()? {
                let _ = context.add_device(dev.syspath());
            }
        }

        Ok(context)
    }

    pub fn monitor(&self) -> Result<Monitor<'_>, Box<dyn error::Error>> {
        let mut monitor = udev::MonitorBuilder::new(&self.udev)?;
        monitor.match_subsystem("usb")?;
        Ok(Monitor {
            context: self,
            socket: monitor.listen()?,
            reg: reactor::Registration::new(),
        })
    }

    fn add_device(&self, path: &Path) -> Option<Id> {
        let dev = self.udev.device_from_syspath(path).ok()?;
        let _ = dev.attribute_value("idVendor")?;
        self.paths.borrow_mut().push(Some(path.to_path_buf()));
        Some(Id((self.paths.borrow().len() - 1) as u32))
    }

    fn remove_device_by_path(&self, path: &Path) -> Option<Id> {
        match self
            .paths
            .borrow_mut()
            .iter_mut()
            .enumerate()
            .find(|(_, current)| current.as_ref().map_or(false, |current| current == path))
        {
            Some((id, path)) => {
                *path = None;
                Some(Id(id as u32))
            }
            None => None,
        }
    }

    // In case Event::Change is used, we'll need to look up Ids by Path.
    fn _find_device_by_path(&self, path: &Path) -> Option<Id> {
        self.paths
            .borrow()
            .iter()
            .enumerate()
            .find_map(|(id, current)| {
                current.as_ref().and_then(|current| {
                    if current == path {
                        Some(Id(id as u32))
                    } else {
                        None
                    }
                })
            })
    }

    fn id(&self, id: Id) -> Result<usize, UsbError> {
        let id = id.into();
        if id < self.paths.borrow().len() {
            let path: &Option<PathBuf> = &self.paths.borrow()[id];
            if path.is_some() {
                Ok(id)
            } else {
                Err(UsbError::NotConnected)
            }
        } else {
            Err(UsbError::InvalidId)
        }
    }

    pub fn is_connected(&self, id: Id) -> bool {
        self.id(id).is_ok()
    }

    fn udev_lookup_hex(&self, id: Id, attr: &str) -> Result<u16, UsbError> {
        fn udev_attribute_walk(dev: &udev::Device, name: &str) -> Option<u16> {
            let attr = dev.attributes().find(|attr| attr.name() == name);
            if let Some(attr) = attr {
                let attr = attr.value()?.to_str()?;
                Some(u16::from_str_radix(attr, 16).ok()?)
            } else {
                udev_attribute_walk(&dev.parent()?, name)
            }
        }

        let id = self.id(id)?;

        // unwrap() is safe here because the above line would have propagated an Err if it was not
        // currently connected.
        let device = self.udev.device_from_syspath(self.paths.borrow()[id].as_ref().unwrap()).map_err(|_| {
            self.paths.borrow_mut()[id] = None;
            UsbError::NotConnected
        })?;
        udev_attribute_walk(&device, attr).ok_or_else(|| {
            self.paths.borrow_mut()[id] = None;
            UsbError::NotConnected
        })
    }

    fn udev_lookup_string(&self, id: Id, attr: &str) -> Result<String, UsbError> {
        fn udev_attribute_walk<'a>(dev: &'a udev::Device, name: &str) -> Option<String> {
            let attr = dev.attributes().find(|attr| attr.name() == name);
            if let Some(attr) = attr {
                Some(String::from(attr.value()?.to_str()?))
            } else {
                udev_attribute_walk(&dev.parent()?, name)
            }
        }

        let id = self.id(id)?;

        let device = self.udev.device_from_syspath(self.paths.borrow()[id].as_ref().unwrap()).map_err(|_| {
            self.paths.borrow_mut()[id] = None;
            UsbError::NotConnected
        })?;
        udev_attribute_walk(&device, attr).ok_or_else(|| {
            self.paths.borrow_mut()[id] = None;
            UsbError::NotConnected
        })
    }

    pub fn vendor_id(&self, id: Id) -> Result<u16, UsbError> {
        self.udev_lookup_hex(id, "idVendor")
    }

    pub fn product_id(&self, id: Id) -> Result<u16, UsbError> {
        self.udev_lookup_hex(id, "idProduct")
    }

    pub fn manufacturer_string(&self, id: Id) -> Result<String, UsbError> {
        self.udev_lookup_string(id, "manufacturer")
    }

    pub fn product_string(&self, id: Id) -> Result<String, UsbError> {
        self.udev_lookup_string(id, "product")
    }

    pub fn devices(&self) -> impl Iterator<Item = Id> {
        (0..(self.paths.borrow().len())).map(|id| Id(id as u32))
    }
}
