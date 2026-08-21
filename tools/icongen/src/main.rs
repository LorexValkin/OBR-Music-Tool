use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (svg_path, out_dir) = (&args[1], &args[2]);
    let data = fs::read(svg_path).expect("read svg");
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&data, &opt).expect("parse svg");
    let size = tree.size();

    let render = |px: u32| -> Vec<u8> {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px).unwrap();
        let (sx, sy) = (px as f32 / size.width(), px as f32 / size.height());
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
        pixmap.encode_png().expect("encode png")
    };

    fs::write(format!("{out_dir}/icon.png"), render(256)).unwrap();

    // ICO with PNG-compressed entries (supported by Windows Vista+).
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| render(s)).collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0u8 } else { *s as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in &images {
        ico.extend_from_slice(img);
    }
    fs::write(format!("{out_dir}/icon.ico"), &ico).unwrap();
    println!("icon.png 256px; icon.ico {} entries, {} bytes", sizes.len(), ico.len());
}
