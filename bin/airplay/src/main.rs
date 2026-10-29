//! Демон AirPlay. По умолчанию все службы работают в этом процессе.

fn main() {
    println!("airplay {}", env!("CARGO_PKG_VERSION"));
}
