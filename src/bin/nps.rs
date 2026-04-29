fn main() {
    if let Err(err) = rustnps::server::entry() {
        rustnps::log_error!("nps", "nps error: {err}");
        std::process::exit(1);
    }
}
