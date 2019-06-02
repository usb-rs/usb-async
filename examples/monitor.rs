use std::error::Error;

use tokio::{
    prelude::*,
    runtime::current_thread,
};
use usb_async;

fn main() -> Result<(), Box<dyn Error>> {
    let ctx = usb_async::Context::new()?;
    let mut mon = ctx.monitor()?.into_future();
    let mut rt = current_thread::Runtime::new()?;
    loop {
        mon = match rt.block_on(mon) {
            Ok((event, chan)) => {
                match event {
                    Some(usb_async::Event::Add(device)) => {
                        let vendor_id = ctx.vendor_id(device).ok_or(usb_async::Error::NotConnected)?;
                        let product_id = ctx.product_id(device).ok_or(usb_async::Error::NotConnected)?;

                        println!("{:04x}:{:04x} was plugged in", vendor_id, product_id);
                    },
                    Some(usb_async::Event::Remove(device)) => {
                        let vendor_id = ctx.vendor_id(device).ok_or(usb_async::Error::NotConnected)?;
                        let product_id = ctx.product_id(device).ok_or(usb_async::Error::NotConnected)?;

                        println!("{:04x}:{:04x} was unplugged", vendor_id, product_id);
                    },
                    None => return Ok(())
                };
                chan
            }
            Err((err, _)) => return Err(Box::new(err)),
        }
        .into_future()
    }
}
