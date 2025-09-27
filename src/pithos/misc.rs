use fast_image_resize::{images::Image, pixels::{U8x3, U8x4}};
use std::{
    fs::File,
    io::{BufWriter, Write},
};

use crate::pithos::commands::RenderMode;

// fast-resized-image into a bufwriter to a file c:
pub fn img_into_buffer(img: &Image, buf: &mut BufWriter<&File>) {
    let start = std::time::Instant::now();
    let loop_start = std::time::Instant::now();

    if let Some(typed) = img.typed_image::<U8x4>() {
        for pixel in typed.pixels() {
            let (r, g, b, a) = (pixel.0[0], pixel.0[1], pixel.0[2], pixel.0[3]);
            buf.write_all(&[b, g, r, a]).unwrap();
        }
    } else {
        if let Some(typed) = img.typed_image::<U8x3>() {
            for pixel in typed.pixels() {
                let (r, g, b, a) = (pixel.0[0], pixel.0[1], pixel.0[2], u8::max_value());
                buf.write_all(&[b, g, r, a]).unwrap();
            }
        } else {
            panic!("image could not be coerced to U8x4");
        }
    }

    let loop_end = std::time::Instant::now();
    buf.flush().unwrap();
    let call_end = std::time::Instant::now();
    println!(
        "[bufwrite] loop [{:?}] total [{:?}]",
        loop_end - loop_start,
        call_end - start
    );
}

// computes the viewport start,end values along an image dimension given a scroll % (0.0 -> 100.0)
pub fn compute_viewport_range(
    image_dimension: i32,
    viewport_dimension: i32,
    scroll_percentage: f64,
) -> (f64, f64) {
    if image_dimension == viewport_dimension {
        return (0.0, image_dimension as f64);
    }
    // given an image dimension (e.g. 5000), a viewport dimension (e.g. 1440), and a scroll percentage (0.0 to 100.0):
    // - i should compute the center of the viewport measured as 0% is half a viewport past the image start
    //   and 100% is half a viewport before the end of the image
    // - i should then return the viewport start & end values, as f64
    // because the viewport 'fixed' value is a 24.8 decimal something or other type with a .into
    let half_viewport = viewport_dimension as f64 / 2.0;
    let start = half_viewport;
    let end = image_dimension as f64 - half_viewport;
    // can be equal: viewport_dimension == image_dimension
    assert!(start <= end);
    let viewport_offset = (end - start) * scroll_percentage / 100.0;
    (
        start + viewport_offset - half_viewport,
        start + viewport_offset + half_viewport,
    )
}

// for when we wanna rescale an image down to fit a given dimension
pub fn get_new_image_dimensions(
    orig_width: u32,
    orig_height: u32,
    output_width: Option<u32>,
    output_height: Option<u32>,
) -> (u32, u32) {
    let scale_factor = match (output_width, output_height) {
        // scale factor is ratio of output to image
        // if image is bigger, it needs to be scaled down
        // if image is smaller, it "needs" to be scaled up
        (Some(canvas_width), None) => canvas_width as f64 / orig_width as f64,
        (None, Some(canvas_height)) => canvas_height as f64 / orig_height as f64,
        (Some(canvas_width), Some(canvas_height)) => {
            // we want to ensure the image covers the entire canvas in all dimensions
            // if image is, say, 100x100 and display is 2000x1000, this gives max(20, 10) => 12 => 2000x2000
            // if image is 10000x10000 and display is 2000x1000, this gives max(0.2, 0.1) => 0.2 => 2000x2000
            // should write tests for this all though
            f64::max(
                canvas_width as f64 / orig_width as f64,
                canvas_height as f64 / orig_height as f64,
            )
        }
        (None, None) => 1_f64,
    };
    (
        (orig_width as f64 * scale_factor).round() as u32,
        (orig_height as f64 * scale_factor).round() as u32,
    )
}

// this implicity enforces viewport "source rectangle" width/height as an inherent property of the
// (canvas image, output mode, render mode) state combination.
// i really really really need to have test coverage of this : )
pub fn get_viewport_dimensions(
    image_width: i32,
    image_height: i32,
    output_width: i32,
    output_height: i32,
    mode: RenderMode,
) -> (i32, i32) {
    // in vertical mode, we want the viewport width to be the entirety of the image width,
    //  then scale the output height by the ratio of output width to image width to get viewport width
    // in lateral mode, we want to transpose all that =)
    // in static mode, we want to 'scale to fit':
    // we pick the min ratio of (image/output) for width | height, & multiply the other output dimension by it (and return the other image dimension)
    let width_ratio = image_width as f64 / output_width as f64;
    let height_ratio = image_height as f64 / output_height as f64;
    match mode {
        RenderMode::Static => {
            // 3440,1440 and 4000,1200
            // we _want_ to do a height of 1200 and a width of (less than 3440)
            // width ratio is: 3440/4000 => 0.86
            // height ratio is: 1200/1440 => 0.83
            // therefore: height is image_height (1200), width is (0.83 * 3440) => 2855ish
            if width_ratio > height_ratio {
                let viewport_width = (height_ratio * output_width as f64).round() as i32;
                let viewport_height = image_height;
                (viewport_width, viewport_height)
            } else if width_ratio < height_ratio {
                let viewport_width = image_width;
                let viewport_height = (width_ratio * output_height as f64).round() as i32;
                (viewport_width, viewport_height)
            } else {
                // image is same aspect ratio as display (dimension scales the same)
                // viewport can be the image dimensions (?)!
                (image_width, image_height)
            }
        }
        RenderMode::ScrollVertical => {
            let viewport_width = image_width;
            let viewport_height = (width_ratio * output_height as f64).round() as i32;
            (viewport_width, viewport_height)
        }
        RenderMode::ScrollLateral => {
            let viewport_width = (height_ratio * output_width as f64).round() as i32;
            let viewport_height = image_height;
            (viewport_width, viewport_height)
        }
    }
}

// i used claude pro to generate these tests, with some nudging about what the expectations were.
// it worked reasonably well and didn't actually take that long, and (after fixing the one test
// that got horizontal and vertical confused), the tests did show my function worked. yippee :)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_mode_wider_image() {
        // Image wider than output (image 4000x1200, output 3440x1440)
        // Should fit by height, resulting viewport maintains output aspect ratio
        let (vw, vh) = get_viewport_dimensions(4000, 1200, 3440, 1440, RenderMode::Static);
        let output_ratio = 3440.0 / 1440.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert_eq!(vh, 1200); // Uses full image height
        assert!(vw <= 4000); // Viewport width <= image width
        assert!(vh <= 1200); // Viewport height <= image height
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_static_mode_taller_image() {
        // Image taller than output (image 1920x2160, output 3440x1440)
        // Should fit by width, resulting viewport maintains output aspect ratio
        let (vw, vh) = get_viewport_dimensions(1920, 2160, 3440, 1440, RenderMode::Static);
        let output_ratio = 3440.0 / 1440.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert_eq!(vw, 1920); // Uses full image width
        assert!(vw <= 1920); // Viewport width <= image width
        assert!(vh <= 2160); // Viewport height <= image height
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_static_mode_same_aspect_ratio() {
        // Image and output have same aspect ratio
        let (vw, vh) = get_viewport_dimensions(3440, 1440, 3440, 1440, RenderMode::Static);
        assert_eq!(vw, 3440);
        assert_eq!(vh, 1440);
        assert!(vw <= 3440);
        assert!(vh <= 1440);

        // Different size but same ratio
        let (vw, vh) = get_viewport_dimensions(1720, 720, 3440, 1440, RenderMode::Static);
        assert_eq!(vw, 1720);
        assert_eq!(vh, 720);
        assert!(vw <= 1720);
        assert!(vh <= 720);
    }

    #[test]
    fn test_scroll_vertical_mode() {
        // In vertical scroll mode, viewport width = image width
        // viewport height scaled to maintain output aspect ratio
        let (vw, vh) = get_viewport_dimensions(2000, 3000, 1920, 1080, RenderMode::ScrollVertical);
        assert_eq!(vw, 2000); // Full image width
        let expected_height = ((2000.0 / 1920.0) * 1080.0) as i32;
        assert_eq!(vh, expected_height);
        assert!(vw <= 2000); // Viewport width <= image width
        assert!(vh <= 3000); // Viewport height <= image height

        let output_ratio = 1920.0 / 1080.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_scroll_lateral_mode() {
        // In lateral scroll mode, viewport height = image height
        // viewport width scaled to maintain output aspect ratio
        let (vw, vh) = get_viewport_dimensions(4000, 1500, 1920, 1080, RenderMode::ScrollLateral);
        assert_eq!(vh, 1500); // Full image height
        let expected_width = 2667;
        assert_eq!(vw, expected_width);
        assert!(vw <= 4000); // Viewport width <= image width
        assert!(vh <= 1500); // Viewport height <= image height

        let output_ratio = 1920.0 / 1080.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_laptop_resolutions_with_web_images() {
        // 1366x768 laptop with typical web image 1920x1080
        let (vw, vh) = get_viewport_dimensions(1920, 1080, 1366, 768, RenderMode::Static);
        assert!(vw <= 1920);
        assert!(vh <= 1080);
        let viewport_ratio = vw as f64 / vh as f64;
        let output_ratio = 1366.0 / 768.0;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);

        // MacBook Air 13" with 4K image
        let (vw, vh) = get_viewport_dimensions(3840, 2160, 1440, 900, RenderMode::Static);
        assert!(vw <= 3840);
        assert!(vh <= 2160);
        let viewport_ratio = vw as f64 / vh as f64;
        let output_ratio = 1440.0 / 900.0;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);

        // Standard FHD laptop with small web image
        let (vw, vh) = get_viewport_dimensions(800, 600, 1920, 1080, RenderMode::Static);
        assert!(vw <= 800);
        assert!(vh <= 600);
        let viewport_ratio = vw as f64 / vh as f64;
        let output_ratio = 1920.0 / 1080.0;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_desktop_resolutions_with_large_images() {
        // 4K monitor with 8K image
        let (vw, vh) = get_viewport_dimensions(7680, 4320, 3840, 2160, RenderMode::Static);
        assert!(vw <= 7680);
        assert!(vh <= 4320);
        let viewport_ratio = vw as f64 / vh as f64;
        let output_ratio = 3840.0 / 2160.0;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);

        // Ultrawide 5K with very large panorama
        let (vw, vh) = get_viewport_dimensions(15000, 3000, 5120, 2160, RenderMode::Static);
        assert!(vw <= 15000);
        assert!(vh <= 3000);
        let viewport_ratio = vw as f64 / vh as f64;
        let output_ratio = 5120.0 / 2160.0;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);

        // 1440p monitor with tall image
        let (vw, vh) = get_viewport_dimensions(2000, 12000, 2560, 1440, RenderMode::Static);
        assert!(vw <= 2000);
        assert!(vh <= 12000);
        let viewport_ratio = vw as f64 / vh as f64;
        let output_ratio = 2560.0 / 1440.0;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_scroll_modes_with_realistic_dimensions() {
        // Vertical scroll: laptop with tall article image
        let (vw, vh) = get_viewport_dimensions(1200, 8000, 1366, 768, RenderMode::ScrollVertical);
        assert_eq!(vw, 1200);
        assert!(vw <= 1200);
        assert!(vh <= 8000);
        let output_ratio = 1366.0 / 768.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);

        // Lateral scroll: ultrawide with wide panorama
        let (vw, vh) = get_viewport_dimensions(12000, 2000, 3440, 1440, RenderMode::ScrollLateral);
        assert_eq!(vh, 2000);
        assert!(vw <= 12000);
        assert!(vh <= 2000);
        let output_ratio = 3440.0 / 1440.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);

        // Vertical scroll: 4K monitor with very tall image
        let (vw, vh) = get_viewport_dimensions(3000, 15000, 3840, 2160, RenderMode::ScrollVertical);
        assert_eq!(vw, 3000);
        assert!(vw <= 3000);
        assert!(vh <= 15000);
        let output_ratio = 3840.0 / 2160.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_ultrawide_with_large_portrait_image() {
        // Ultrawide 3440x1440 with large portrait image 7921x11279 in vertical scroll mode
        let (vw, vh) = get_viewport_dimensions(7921, 11279, 3440, 1440, RenderMode::ScrollVertical);
        assert_eq!(vw, 7921); // Full image width
        assert!(vw <= 7921); // Viewport width <= image width
        assert!(vh <= 11279); // Viewport height <= image height

        let expected_height = 3316;
        assert_eq!(vh, expected_height);

        let output_ratio = 3440.0 / 1440.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_extreme_aspect_ratios() {
        // Very wide image with normal output
        let (vw, vh) = get_viewport_dimensions(8000, 100, 1920, 1080, RenderMode::Static);
        let output_ratio = 1920.0 / 1080.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!(vw <= 8000);
        assert!(vh <= 100);
        assert!((viewport_ratio - output_ratio).abs() < 0.01);

        // Very tall image with normal output
        let (vw, vh) = get_viewport_dimensions(100, 8000, 1920, 1080, RenderMode::Static);
        let output_ratio = 1920.0 / 1080.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!(vw <= 100);
        assert!(vh <= 8000);
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }

    #[test]
    fn test_square_image_with_widescreen_output() {
        // Square image (1000x1000) with widescreen output (1920x1080)
        let (vw, vh) = get_viewport_dimensions(1000, 1000, 1920, 1080, RenderMode::Static);
        assert_eq!(vw, 1000); // Uses full image width
        let expected_height = 563;
        assert_eq!(vh, expected_height);
        assert!(vw <= 1000); // Viewport width <= image width
        assert!(vh <= 1000); // Viewport height <= image height

        let output_ratio = 1920.0 / 1080.0;
        let viewport_ratio = vw as f64 / vh as f64;
        assert!((viewport_ratio - output_ratio).abs() < 0.01);
    }
}
