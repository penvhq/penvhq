use std::io::IsTerminal;

use penv::cli::Cli;
use penv::commands;
use penv::env::Env;
use penv::error::Exit;
use penv::output::{Output, resolve};

fn main() {
    penv::upgrade::sweep_retired();
    let cli = Cli::parse_checked();
    let env = Env::from_process();
    let person = !penv::agent::detect_here(&env, true).is_agent();
    let render = resolve(
        cli.json,
        cli.format,
        cli.agent || !person,
        std::io::stdout().is_terminal(),
        &env,
    );
    penv::ui::init(
        !render.json
            && person
            && std::io::stdout().is_terminal()
            && std::io::stderr().is_terminal(),
        render.color,
    );
    let out = Output::new(render);

    let cwd = std::env::current_dir().unwrap_or_default();
    let exit = match commands::dispatch(&cli, &out, &cwd, &env) {
        Ok(report) => {
            let _ = out.write(&report, &mut std::io::stdout());
            report.exit
        }
        Err(error) => {
            let _ = out.fail(&error, &mut std::io::stderr());
            error.exit
        }
    };
    if exit != Exit::Ok {
        std::process::exit(exit as i32);
    }
}
