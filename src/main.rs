mod cli;
mod manual;
mod settings;
mod translation;
mod tui;

fn main() -> anyhow::Result<()> {
    cli::run()
}
