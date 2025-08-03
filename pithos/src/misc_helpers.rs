use std::{fs::File, io::Write};

use image::RgbaImage;

pub fn img_into_buffer(img: &RgbaImage, f: &File) {
    let mut buf = std::io::BufWriter::new(f);
    for pixel in img.pixels() {
        let (r, g, b, a) = (pixel.0[0], pixel.0[1],pixel.0[2],pixel.0[3]);
        buf.write_all(&[b as u8, g as u8, r as u8, a as u8]).unwrap();
    }
}