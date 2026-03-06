use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use png::{BitDepth, ColorType, Encoder};
use serde::Serialize;
use std::error::Error;
use xcap::Monitor;

#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotResult {
    pub width: u32,
    pub height: u32,
    pub monitor: Option<String>,
    pub png_base64: String,
}

fn encode_png_base64(width: u32, height: u32, rgba: &[u8]) -> Result<String, Box<dyn Error>> {
    let mut bytes = Vec::new();
    let mut encoder = Encoder::new(&mut bytes, width, height);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    drop(writer);

    Ok(STANDARD.encode(bytes))
}

pub fn capture_all_screenshots() -> Result<Vec<ScreenshotResult>, Box<dyn Error>> {
    let monitors = Monitor::all()?;
    if monitors.is_empty() {
        return Err("no monitor available for screenshot capture".into());
    }

    let mut screenshots = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        let monitor_name = monitor.name().ok();
        let image = monitor.capture_image()?;
        let width = image.width();
        let height = image.height();
        let rgba = image.into_raw();

        screenshots.push(ScreenshotResult {
            width,
            height,
            monitor: monitor_name,
            png_base64: encode_png_base64(width, height, &rgba)?,
        });
    }

    Ok(screenshots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_png_base64_generates_png_data() {
        let encoded = encode_png_base64(1, 1, &[255, 0, 0, 255]).expect("encode png");
        let decoded = STANDARD.decode(encoded).expect("decode base64");

        assert!(decoded.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
    }

    #[test]
    fn encode_png_base64_rejects_invalid_rgba_length() {
        let result = encode_png_base64(1, 1, &[255, 0, 0]);

        assert!(result.is_err());
    }
}
