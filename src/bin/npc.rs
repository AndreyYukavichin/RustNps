fn main() {
    if let Err(err) = rustnps::client::entry() {
        rustnps::log_error!("npc", "npc error: {err}");
        std::process::exit(1);
    }
}
