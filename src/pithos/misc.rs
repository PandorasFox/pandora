use std::{fs::File, io::Write};
use image::RgbaImage;

pub fn img_into_buffer(img: &RgbaImage, f: &File) {
    let mut buf = std::io::BufWriter::new(f);
    // this could potentially be SIMD'd or otherwise accelerated, I think
    // definitely a bottleneck in the load
    for pixel in img.pixels() {
        let (r, g, b, a) = (pixel.0[0], pixel.0[1],pixel.0[2],pixel.0[3]);
        buf.write_all(&[b as u8, g as u8, r as u8, a as u8]).unwrap();
    }
}

pub fn get_new_image_dimensions(orig_width: u32, orig_height: u32, output_width: Option<u32>, output_height: Option<u32>) -> (u32, u32) {
    let scale_factor = match (output_width, output_height) {
        // scale factor is ratio of output to image
        // if image is bigger, it needs to be scaled down
        // if image is smaller, it "needs" to be scaled up
        (Some(canvas_width), None) => {
            canvas_width as f64 / orig_width as f64
        }, 
        (None, Some(canvas_height)) => {
            canvas_height as f64 / orig_height as f64
        },
        (Some(canvas_width), Some(canvas_height)) => {
            // we want to ensure the image covers the entire canvas in all dimensions
            // if image is, say, 100x100 and display is 2000x1000, this gives max(20, 10) => 12 => 2000x2000 
            // if image is 10000x10000 and display is 2000x1000, this gives max(0.2, 0.1) => 0.2 => 2000x2000
            // should write tests for this all though
            f64::max(
                canvas_width as f64 / orig_width as f64,
                canvas_height as f64 / orig_height as f64,
            )
        },
        (None, None) => {
            1 as f64
        }
    };
    return ((orig_width as f64 * scale_factor).round() as u32, (orig_height as f64 * scale_factor).round() as u32);
}

// TODO write some tests over that ^