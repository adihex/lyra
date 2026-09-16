fn main() {
    std::process::exit(lyra_cli::cli::run(std::env::args().collect()));
}
