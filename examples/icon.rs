//! Renders assets/icon.svg into the macOS iconset and the runtime window icon.
//! `cargo run --example icon` (used by scripts/bundle.sh).

fn main() {
    let svg = std::fs::read("assets/icon.svg").expect("assets/icon.svg");
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).expect("valid svg");
    let render = |px: u32, path: &str| {
        let mut pm = resvg::tiny_skia::Pixmap::new(px, px).unwrap();
        let s = px as f32 / 1024.0;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pm.as_mut());
        pm.save_png(path).expect("write png");
    };
    let set = "target/DataTired.iconset";
    std::fs::create_dir_all(set).unwrap();
    for size in [16u32, 32, 128, 256, 512] {
        render(size, &format!("{set}/icon_{size}x{size}.png"));
        render(size * 2, &format!("{set}/icon_{size}x{size}@2x.png"));
    }
    render(256, "assets/icon.png");
}
