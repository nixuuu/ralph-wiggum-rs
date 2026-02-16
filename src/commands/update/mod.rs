pub fn execute() {
    crate::shared::banner::print_banner();
    if let Err(e) = crate::updater::update_self() {
        let msg = format!("Update failed: {e}");
        println!("{}", msg);
        crate::diag_warn!("{}", msg);
        std::process::exit(1);
    }
    std::process::exit(0);
}
