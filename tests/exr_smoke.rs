use image::{DynamicImage, Rgba, Rgba32FImage};

#[test]
fn exr_decodes_as_linear_float_and_keeps_values_above_one() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join("smoke.exr");

    let mut img = Rgba32FImage::new(4, 3);
    img.put_pixel(0, 0, Rgba([8.0, 0.5, 0.25, 1.0])); // deliberately > 1.0
    DynamicImage::ImageRgba32F(img).save(&path).unwrap();

    let decoded = image::open(&path).unwrap();
    assert!(
        matches!(
            decoded,
            DynamicImage::ImageRgba32F(_) | DynamicImage::ImageRgb32F(_)
        ),
        "expected a float variant, got {:?}",
        decoded.color()
    );

    let f = decoded.to_rgba32f();
    assert_eq!(f.as_raw().len(), 4 * 3 * 4, "rgba32f element count");
    assert_eq!(
        bytemuck::cast_slice::<f32, u8>(f.as_raw()).len(),
        4 * 3 * 16,
        "byte length must match bytes_per_pixel = 16"
    );
    assert!(
        f.get_pixel(0, 0)[0] > 7.0,
        "highlight above 1.0 was clipped"
    );

    // What the old path did, for contrast.
    assert_eq!(
        decoded.to_rgba8().get_pixel(0, 0)[0],
        255,
        "clipped to white"
    );
}
