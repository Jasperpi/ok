//! Writes a QR code as a PNG and Gallant as a TTF, for checking by eye and
//! with other programs: `cargo run -p ok-link --example proof -- out_dir text`.
use ok_gfx::{encode_png, Frame, Rect, Rgb};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "shots".into()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, text) in args.iter().skip(1).enumerate() {
        let qr = ok_link::qr::encode(text.as_bytes()).expect("fits");
        let scale = 8;
        let side = (qr.size + 8) * scale;
        let mut frame = Frame::new(side, side);
        frame.clear(Rgb(255, 255, 255));
        for y in 0..qr.size {
            for x in 0..qr.size {
                if qr.dark(x, y) {
                    frame.fill(Rect::new(((x + 4) * scale) as i32, ((y + 4) * scale) as i32, scale as i32, scale as i32), Rgb(0, 0, 0));
                }
            }
        }
        std::fs::write(dir.join(format!("qr{i}.png")), encode_png(side, side, frame.pixels())).unwrap();
    }
    std::fs::write(dir.join("gallant.ttf"), ok_link::ttf::build(ok_gfx::gallant(), "ok Gallant")).unwrap();
}
