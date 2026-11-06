//! Writes the relay's static files: the phone page, its font, the icon and
//! the web app manifest. `cargo run -p ok-link --example relay-assets -- relay/public`
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "relay/public".into()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), ok_link::page().replace("{{RELAY}}", "")).unwrap();
    std::fs::write(dir.join("gallant.ttf"), ok_link::font()).unwrap();
    for size in [192, 512] {
        let icon = ok_gfx::icon(size);
        std::fs::write(dir.join(format!("icon-{size}.png")), ok_gfx::encode_png(icon.width(), icon.height(), icon.pixels())).unwrap();
    }
    std::fs::copy(dir.join("icon-192.png"), dir.join("icon.png")).unwrap();
    // The size an iPhone asks for on its home screen.
    let apple = ok_gfx::icon(180);
    std::fs::write(dir.join("apple-touch-icon.png"), ok_gfx::encode_png(apple.width(), apple.height(), apple.pixels())).unwrap();
    std::fs::write(
        dir.join("manifest.webmanifest"),
        r##"{"name":"ok","short_name":"ok","start_url":"/","scope":"/","display":"standalone","background_color":"#35251A","theme_color":"#35251A","icons":[{"src":"/icon-192.png","sizes":"192x192","type":"image/png"},{"src":"/icon-512.png","sizes":"512x512","type":"image/png","purpose":"any"}]}"##,
    )
    .unwrap();
    std::fs::write(
        dir.join("_headers"),
        "/*\n  X-Content-Type-Options: nosniff\n  Referrer-Policy: no-referrer\n  Permissions-Policy: microphone=(self), camera=()\n/\n  Cache-Control: no-cache\n/index.html\n  Cache-Control: no-cache\n",
    )
    .unwrap();
    println!("wrote {}", dir.display());
}
