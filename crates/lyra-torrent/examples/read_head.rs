use lyra_fs::ByteSource;
use std::sync::Arc;
fn main() {
    let eng = Arc::new(
        lyra_torrent::TorrentEngine::new(std::env::temp_dir().join("lyra-readhead")).unwrap(),
    );
    let spec = std::env::args().nth(1).unwrap();
    let idx: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let id = eng.add(&spec).expect("add failed");
    println!("added id={id}, opening file {idx}");
    let src = eng.open_file(id, idx).expect("open_file failed");
    let mut buf = vec![0u8; 64 * 1024];
    let n = src.read_at(0, &mut buf).expect("read_at failed");
    println!("read {n} bytes; head: {:02x?}", &buf[..16.min(n)]);
    println!("as ascii: {}", String::from_utf8_lossy(&buf[..16.min(n)]));
}
