use std::sync::Arc;
fn main() {
    let eng = Arc::new(lyra_torrent::TorrentEngine::new(
        std::env::temp_dir().join("lyra-tryadd"),
    ).unwrap());
    let m = std::env::args().nth(1).unwrap();
    match eng.add(&m) {
        Ok(id) => println!("added id={id} files={:?}", eng.files(id).map(|f| f.len())),
        Err(e) => println!("ADD FAILED: {e}"),
    }
}
