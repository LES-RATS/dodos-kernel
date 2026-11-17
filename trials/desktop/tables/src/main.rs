//! Usage: dodos-trial-desktop-tables RESULTS.TXT [more result files...]
//! Writes the Markdown report to standard output.

fn main() {
    let mut text = String::new();
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        text.push_str(&String::from_utf8_lossy(&bytes));
        text.push('\n');
    }
    print!("{}", dodos_trial_desktop_tables::report(&text));
}
