use std::io::IsTerminal;

use penv::cli::Cli;
use penv::commands;
use penv::env::Env;
use penv::error::Exit;
use penv::output::{Output, resolve};

fn main() {
    // The resolver's depth limits are sized to this stack; Windows gives the
    // main thread only 1 MiB.
    let worker = std::thread::Builder::new()
        .stack_size(penv_schema::resolve::STACK)
        .spawn(penv_main)
        .unwrap_or_else(|e| {
            eprintln!("penv could not start: {e}");
            std::process::exit(1)
        });
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
}

fn penv_main() {
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
