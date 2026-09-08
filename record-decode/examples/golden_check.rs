fn main() {
    let root = std::env::args().nth(1).expect("root");
    for (rel, profile) in [
        ("goldenfiles/records/lori-asha-westside-single45-hq/lori-asha-westside-single45-hq.record.png", "single45"),
        ("goldenfiles/records/lori-asha-westside-lp-hq/lori-asha-westside-lp-hq.record.png", "lp"),
    ] {
        let bytes = std::fs::read(format!("{root}/{rel}")).expect("golden png");
        match record_decode::decode_record_descriptor_from_png(&bytes, Some(profile)) {
            Ok(_) => println!("{profile:10} DECODES"),
            Err(e) => println!("{profile:10} BROKEN: {}", format!("{e:#}")),
        }
    }
}
