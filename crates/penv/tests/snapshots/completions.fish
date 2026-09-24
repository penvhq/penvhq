# penv completions fish
function __fish_penv_path --description 'the penv command path typed so far'
    set -l tokens (commandline -opc)
    if set -q tokens[1]
        set -e tokens[1]
    end
    set -l path
    set -l skip 0
    for token in $tokens
        if test $skip -eq 1
            set skip 0
        else if test "$token" = '--'
            break
        else if string match -q -- '-*' $token
            if contains -- $token --format --provider --guards --output --env --org --value --out --approval
                set skip 1
            end
        else
            set -a path $token
        end
    end
    string join ' ' $path
end

function __fish_penv_at --description 'true at one penv command path'
    set -l path (__fish_penv_path)
    test "$path" = "$argv[1]"
end

complete -c penv -f
complete -c penv -l json -d 'Emit JSON on stdout, whatever stdout is attached to'
complete -c penv -l format -r -d 'Pick the output format: json or text' -a 'json text'
complete -c penv -l agent -d 'Treat this session as an agent: JSON out, values masked'
complete -c penv -l provider -r -d 'Read values from this provider instead of the one @penv= names'
complete -c penv -n '__fish_penv_at ""' -a 'init' -d 'Read .env, write .env.schema, and keep .env out of the repository'
complete -c penv -n '__fish_penv_at ""' -a 'run' -d 'Run a command with your secrets loaded into it'
complete -c penv -n '__fish_penv_at ""' -a 'push' -d 'Move local values to the cloud and delete .env'
complete -c penv -n '__fish_penv_at ""' -a 'pull' -d 'Write a plain .env from the cloud'
complete -c penv -n '__fish_penv_at ""' -a 'login' -d 'Sign in through your browser; the login is kept in your system'\''s password store'
complete -c penv -n '__fish_penv_at ""' -a 'logout' -d 'Sign out on this machine'
complete -c penv -n '__fish_penv_at ""' -a 'set' -d 'Save one value (typed hidden, never shown)'
complete -c penv -n '__fish_penv_at ""' -a 'unset' -d 'Delete one value'
complete -c penv -n '__fish_penv_at ""' -a 'ls' -d 'List your keys and show which ones have a value'
complete -c penv -n '__fish_penv_at ""' -a 'bundle' -d 'Write one environment'\''s values, encrypted, to .penv/<env>.bundle for a deploy'
complete -c penv -n '__fish_penv_at ""' -a 'encrypt' -d 'Encrypt the sensitive values in the .env files beside .env.schema'
complete -c penv -n '__fish_penv_at ""' -a 'decrypt' -d 'Write the encrypted values in the .env files back in plain text'
complete -c penv -n '__fish_penv_at ""' -a 'why' -d 'Say where a key'\''s value comes from and how penv treats it, never the value'
complete -c penv -n '__fish_penv_at ""' -a 'check' -d 'Report schema problems and missing values'
complete -c penv -n '__fish_penv_at ""' -a 'gen' -d 'Write the typed file for your language (ts, py, go, rust, php, java, csharp)'
complete -c penv -n '__fish_penv_at ""' -a 'scan' -d 'Find secret values committed to files'
complete -c penv -n '__fish_penv_at ""' -a 'guard' -d 'Write the harness rules that keep agents out of .env'
complete -c penv -n '__fish_penv_at ""' -a 'reveal' -d 'Show one value; an AI agent needs your approval first'
complete -c penv -n '__fish_penv_at ""' -a 'project' -d 'Your projects: ls, new, rename, rm'
complete -c penv -n '__fish_penv_at ""' -a 'env' -d 'A project'\''s environments: ls, new, rename, copy, rm'
complete -c penv -n '__fish_penv_at ""' -a 'machine' -d 'Identities for servers and CI'
complete -c penv -n '__fish_penv_at ""' -a 'upgrade' -d 'Replace penv with the latest release'
complete -c penv -n '__fish_penv_at ""' -a 'completions' -d 'Print the shell completion script'
complete -c penv -n '__fish_penv_at ""' -a 'hook' -d 'Run as a harness hook; a payload it cannot read is refused'
complete -c penv -n '__fish_penv_at ""' -a 'schema' -d 'Print the schema as JSON'
complete -c penv -n '__fish_penv_at ""' -a 'help' -d 'Show help for a command'
complete -c penv -n '__fish_penv_at ""' -l json -d 'Emit JSON on stdout, whatever stdout is attached to'
complete -c penv -n '__fish_penv_at ""' -l format -r -d 'Pick the output format: json or text' -a 'json text'
complete -c penv -n '__fish_penv_at ""' -l agent -d 'Treat this session as an agent: JSON out, values masked'
complete -c penv -n '__fish_penv_at ""' -l provider -r -d 'Read values from this provider instead of the one @penv= names'
complete -c penv -n '__fish_penv_at "init"' -l force -d 'Overwrite an existing .env.schema'
complete -c penv -n '__fish_penv_at "init"' -l guards -r -d 'Guard exactly these harnesses instead of the installed ones'
complete -c penv -n '__fish_penv_at "init"' -l no-guards -d 'Write no harness rules at all'
complete -c penv -n '__fish_penv_at "init"' -l output -r -d 'Write the generated typed file here, relative to the repository root'
complete -c penv -n '__fish_penv_at "run"' -F -a '(__fish_complete_command)'
complete -c penv -n '__fish_penv_at "run"' -l env -r -d 'The environment to read'
complete -c penv -n '__fish_penv_at "run"' -l no-mask -d 'Show secrets in the command'\''s output instead of hiding them'
complete -c penv -n '__fish_penv_at "run"' -l no-preload -d 'Do not load penv'\''s masking into the command'\''s runtime (Node, Bun, Deno, Python)'
complete -c penv -n '__fish_penv_at "run"' -l sealed -d 'Give keys with @hosts to the command as placeholders; penv puts the values into requests to those hosts. Always on for an AI agent'
complete -c penv -n '__fish_penv_at "push"' -l env -r -d 'The environment to write to'
complete -c penv -n '__fish_penv_at "push"' -l org -r -d 'The organisation that owns a project penv is about to create'
complete -c penv -n '__fish_penv_at "push"' -l prune -d 'Delete cloud keys the schema no longer lists'
complete -c penv -n '__fish_penv_at "pull"' -l env -r -d 'The environment to read'
complete -c penv -n '__fish_penv_at "pull"' -l i-am-human -d 'Confirm a person, not an agent, asked for the file'
complete -c penv -n '__fish_penv_at "set"' -l env -r -d 'The environment to write to'
complete -c penv -n '__fish_penv_at "set"' -l value -r -d 'Refused: a value passed here lands in the shell history'
complete -c penv -n '__fish_penv_at "unset"' -l env -r -d 'The environment to write to'
complete -c penv -n '__fish_penv_at "ls"' -l env -r -d 'The environment to read'
complete -c penv -n '__fish_penv_at "bundle"' -l env -r -d 'The environment to bundle'
complete -c penv -n '__fish_penv_at "why"' -l env -r -d 'The environment to read'
complete -c penv -n '__fish_penv_at "check"' -l env -r -d 'The environment to check'
complete -c penv -n '__fish_penv_at "check"' -l strict -d 'Fail when code reads a variable .env.schema does not declare'
complete -c penv -n '__fish_penv_at "gen"' -l out -r -d 'Write here instead, relative to the repository root'
complete -c penv -n '__fish_penv_at "gen"' -l check -d 'Compare with what is on disk instead of writing'
complete -c penv -n '__fish_penv_at "gen"' -l options -d 'Show what this target'\''s options change instead of writing'
complete -c penv -n '__fish_penv_at "scan"' -l staged -d 'Scan only what is staged for the next commit'
complete -c penv -n '__fish_penv_at "scan"' -l install-hook -d 'Write a git pre-commit hook that runs penv scan --staged'
complete -c penv -n '__fish_penv_at "scan"' -l env -r -d 'The environment whose values to look for'
complete -c penv -n '__fish_penv_at "guard"' -l all -d 'Write every harness penv knows, installed or not'
complete -c penv -n '__fish_penv_at "guard"' -l check -d 'Report coverage instead of writing'
complete -c penv -n '__fish_penv_at "reveal"' -l env -r -d 'The environment to read'
complete -c penv -n '__fish_penv_at "reveal"' -l approval -r -d 'Print the value a person approved under this id'
complete -c penv -n '__fish_penv_at "project"' -a 'ls' -d 'List your projects and their environments'
complete -c penv -n '__fish_penv_at "project"' -a 'new' -d 'Create a project with a development environment'
complete -c penv -n '__fish_penv_at "project"' -a 'rename' -d 'Rename a project; a folder linked to it is updated too'
complete -c penv -n '__fish_penv_at "project"' -a 'rm' -d 'Delete a project and every value in it, for good'
complete -c penv -n '__fish_penv_at "project ls"' -l org -r -d 'Only this organisation'
complete -c penv -n '__fish_penv_at "project new"' -l org -r -d 'The organisation that owns it'
complete -c penv -n '__fish_penv_at "project rename"' -l org -r -d ''
complete -c penv -n '__fish_penv_at "project rm"' -l org -r -d ''
complete -c penv -n '__fish_penv_at "env"' -a 'ls' -d 'List the project'\''s environments'
complete -c penv -n '__fish_penv_at "env"' -a 'new' -d 'Create an empty environment'
complete -c penv -n '__fish_penv_at "env"' -a 'rename' -d 'Rename an environment'
complete -c penv -n '__fish_penv_at "env"' -a 'copy' -d 'Create an environment with another one'\''s keys; values are never copied'
complete -c penv -n '__fish_penv_at "env"' -a 'rm' -d 'Delete an environment and every value in it, for good'
complete -c penv -n '__fish_penv_at "machine"' -a 'enroll' -d 'Give this server its own identity, from a one-time secret made in the console'
complete -c penv -n '__fish_penv_at "upgrade"' -l check -d 'Only report what that release is'
complete -c penv -n '__fish_penv_at "completions"' -a 'bash zsh fish powershell elvish'
