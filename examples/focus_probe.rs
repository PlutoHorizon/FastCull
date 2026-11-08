//! Read-only metadata verification against real photographs.
fn main() -> anyhow::Result<()> {
    for path in std::env::args_os().skip(1) {
        let path = std::path::Path::new(&path);
        println!(
            "{}: {:#?}",
            path.display(),
            fastcull::photo_focus::read(path)?
        );
    }
    Ok(())
}
