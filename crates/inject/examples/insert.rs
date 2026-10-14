//! `cargo run -p whidictate-inject --example insert -- "Text"`
//! Wartet 4 Sekunden (Zeit, in Word/Outlook/Editor zu klicken) und fügt dann den Text ein.
fn main() -> anyhow::Result<()> {
    let text = std::env::args().nth(1).unwrap_or_else(|| "Müller, Straße – „Größe“ § 823 BGB".into());
    let mut inj = whidictate_inject::Injector::new(whidictate_inject::Method::Paste)?;
    eprintln!("Klicke jetzt in das Zielprogramm ...");
    std::thread::sleep(std::time::Duration::from_secs(4));
    inj.insert(&text)?;
    eprintln!("eingefügt");
    Ok(())
}
