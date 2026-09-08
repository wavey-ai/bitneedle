fn main() {
    let path = std::env::args().nth(1).expect("png");
    let img = image::open(&path).expect("png").to_rgba8();
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
    println!("{w}x{h}, centre ({cx}, {cy})");
    // Count opaque pixels in each 1px annulus near the edge.
    for r in 274..=288 {
        let (mut opaque, mut total) = (0usize, 0usize);
        let mut sample = None;
        for y in 0..h {
            for x in 0..w {
                let d = (((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)) as f64).sqrt();
                if d >= r as f64 && d < (r + 1) as f64 {
                    total += 1;
                    let p = img.get_pixel(x as u32, y as u32).0;
                    if p[3] > 0 {
                        opaque += 1;
                        if sample.is_none() { sample = Some(p); }
                    }
                }
            }
        }
        println!("r={r:3}  opaque {opaque:5}/{total:5}  sample {:?}", sample);
    }
}
