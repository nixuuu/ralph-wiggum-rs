pub fn execute() {
    crate::shared::banner::print_banner();
    if let Err(e) = crate::updater::update_self() {
        eprintln!("Update failed: {e}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
