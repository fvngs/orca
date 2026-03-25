mod app;
mod docker;
mod events;
mod ui;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;

use app::App;
use docker::DockerBackend;

#[derive(Parser)]
#[command(name = "orca", about = "A TUI for Docker and Podman")]
struct Cli {
    /// Docker host / socket (e.g. unix:///run/user/1000/podman/podman.sock)
    #[arg(long, short = 'H')]
    host: Option<String>,

    /// Force use of Podman socket
    #[arg(long)]
    podman: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let backend = DockerBackend::connect(cli.host.as_deref(), cli.podman)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let term_backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(term_backend)?;

    let result = run(&mut terminal, backend).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    backend: DockerBackend,
) -> Result<()> {
    let mut app = App::new(backend);
    app.start_background_tasks();

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        if events::handle_events(&mut app).await? {
            break;
        }
    }

    app.shutdown().await;
    Ok(())
}
