#compdef penv
# penv completions zsh

_penv() {
    local -a commands
    commands=(
        'init:Read .env, write .env.schema, and keep .env out of the repository'
        'run:Run a command with your secrets loaded into it'
        'push:Move local values to the cloud and delete .env'
        'pull:Write a plain .env from the cloud'
        'login:Sign in through your browser; the login is kept in your system'\''s password store'
        'logout:Sign out on this machine'
        'set:Save one value (typed hidden, never shown)'
        'unset:Delete one value'
        'ls:List your keys and show which ones have a value'
        'check:Report schema problems and missing values'
        'gen:Write the typed file for your language (ts, py)'
        'guard:Write the harness rules that keep agents out of .env'
        'reveal:Show one value; an AI agent needs your approval first'
        'project:Your projects: ls, new, rename, rm'
        'env:A project'\''s environments: ls, new, rename, copy, rm'
        'machine:Identities for servers and CI'
        'upgrade:Replace penv with the latest release'
        'completions:Print the shell completion script'
        'hook:Run as a harness hook; a payload it cannot read is refused'
        'schema:Print the schema as JSON'
        'help:Show help for a command'
    )
    local curcontext="$curcontext" state line
    _arguments -C \
        '--json[Emit JSON on stdout, whatever stdout is attached to]' \
        '--format[Pick the output format: json or text]:value:(json text)' \
        '--agent[Treat this session as an agent: JSON out, values masked]' \
        '1: :->command' \
        '*:: :->args' && return 0

    case $state in
        command) _describe -t commands 'penv command' commands ;;
        args)
            case $words[1] in
                init) _arguments '--force[Overwrite an existing .env.schema]' '--guards[Guard exactly these harnesses instead of the installed ones]:value:' '--no-guards[Write no harness rules at all]' '--output[Write the generated typed file here, relative to the repository root]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                run) _arguments '--env[The environment to read]:value:' '--no-mask[Show secrets in the command'\''s output instead of hiding them]' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' '*:command:_command_names -e' ;;
                push) _arguments '--env[The environment to write to]:value:' '--org[The organisation that owns a project penv is about to create]:value:' '--prune[Delete cloud keys the schema no longer lists]' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                pull) _arguments '--env[The environment to read]:value:' '--i-am-human[Confirm a person, not an agent, asked for the file]' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                login) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                logout) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                set) _arguments '--env[The environment to write to]:value:' '--value[Refused: a value passed here lands in the shell history]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                unset) _arguments '--env[The environment to write to]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                ls) _arguments '--env[The environment to read]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                check) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                gen) _arguments '--out[Write here instead, relative to the repository root]:value:' '--check[Compare with what is on disk instead of writing]' '--options[Show what this target'\''s options change instead of writing]' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                guard) _arguments '--all[Write every harness penv knows, installed or not]' '--check[Report coverage instead of writing]' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                reveal) _arguments '--env[The environment to read]:value:' '--approval[Print the value a person approved under this id]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                project) _penv_project ;;
                env) _penv_env ;;
                machine) _penv_machine ;;
                upgrade) _arguments '--check[Only report what the latest release is]' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                completions) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' '1: :(bash zsh fish powershell elvish)' ;;
                hook) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                schema) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                help) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                *) ;;
            esac ;;
    esac
}

_penv_project() {
    local -a commands
    commands=(
        'ls:List your projects and their environments'
        'new:Create a project with a development environment'
        'rename:Rename a project; a folder linked to it is updated too'
        'rm:Delete a project and every value in it, for good'
    )
    local curcontext="$curcontext" state line
    _arguments -C \
        '--json[Emit JSON on stdout, whatever stdout is attached to]' \
        '--format[Pick the output format: json or text]:value:(json text)' \
        '--agent[Treat this session as an agent: JSON out, values masked]' \
        '1: :->command' \
        '*:: :->args' && return 0

    case $state in
        command) _describe -t commands 'project command' commands ;;
        args)
            case $words[1] in
                ls) _arguments '--org[Only this organisation]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                new) _arguments '--org[The organisation that owns it]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                rename) _arguments '--org[]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                rm) _arguments '--org[]:value:' '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                *) ;;
            esac ;;
    esac
}

_penv_env() {
    local -a commands
    commands=(
        'ls:List the project'\''s environments'
        'new:Create an empty environment'
        'rename:Rename an environment'
        'copy:Create an environment with another one'\''s keys; values are never copied'
        'rm:Delete an environment and every value in it, for good'
    )
    local curcontext="$curcontext" state line
    _arguments -C \
        '--json[Emit JSON on stdout, whatever stdout is attached to]' \
        '--format[Pick the output format: json or text]:value:(json text)' \
        '--agent[Treat this session as an agent: JSON out, values masked]' \
        '1: :->command' \
        '*:: :->args' && return 0

    case $state in
        command) _describe -t commands 'env command' commands ;;
        args)
            case $words[1] in
                ls) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                new) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                rename) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                copy) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                rm) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                *) ;;
            esac ;;
    esac
}

_penv_machine() {
    local -a commands
    commands=(
        'enroll:Give this server its own identity, from a one-time secret made in the console'
    )
    local curcontext="$curcontext" state line
    _arguments -C \
        '--json[Emit JSON on stdout, whatever stdout is attached to]' \
        '--format[Pick the output format: json or text]:value:(json text)' \
        '--agent[Treat this session as an agent: JSON out, values masked]' \
        '1: :->command' \
        '*:: :->args' && return 0

    case $state in
        command) _describe -t commands 'machine command' commands ;;
        args)
            case $words[1] in
                enroll) _arguments '--json[Emit JSON on stdout, whatever stdout is attached to]' '--format[Pick the output format: json or text]:value:(json text)' '--agent[Treat this session as an agent: JSON out, values masked]' ;;
                *) ;;
            esac ;;
    esac
}

_penv "$@"
