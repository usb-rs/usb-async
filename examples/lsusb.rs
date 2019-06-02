use std::error::Error;

use usb_async;

fn main() -> Result<(), Box<dyn Error>> {
    let ctx = usb_async::Context::new()?;

    for dev in ctx.connected_devices() {
        let vendor_id = ctx.vendor_id(dev).ok_or(usb_async::Error::NotConnected)?;
        let product_id = ctx.product_id(dev).ok_or(usb_async::Error::NotConnected)?;
        let manufacturer_string = ctx.manufacturer_string(dev)?;
        let product_string = ctx.product_string(dev)?;
        println!(
            "{:04x}:{:04x} {} {}",
            vendor_id, product_id, manufacturer_string, product_string
        );
    }

    Ok(())
}
