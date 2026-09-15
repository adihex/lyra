// seeds the app's sandboxed library DB from the real fixture dir
use lyra_store::Library;
use std::path::Path;
fn main() {
    let db = Path::new("/Users/adityabalakrishnan/Library/Containers/app.lyra.player/Data/Library/Application Support/Lyra/library.db");
    let lib = Library::open(db).unwrap();
    let stats = lib.sync_dir(Path::new("/Users/adityabalakrishnan/Music/Lyra-Test")).unwrap();
    println!("{stats:?}");
    println!("tracks: {}", lib.all_tracks().unwrap().len());
}
