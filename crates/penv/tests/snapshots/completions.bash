# penv completions bash
_penv() {
    local cur prev cmd word skip i words system
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    cmd=""
    skip=""
    for ((i = 1; i < COMP_CWORD; i++)); do
        word="${COMP_WORDS[i]}"
        if [ -n "$skip" ]; then skip=""; continue; fi
        case "$word" in
            --format|--provider|--guards|--output|--env|--org|--value|--out|--approval) skip=1; continue ;;
            -*) continue ;;
        esac
        case "${cmd:+$cmd }$word" in
            init|run|push|pull|login|logout|set|unset|ls|bundle|encrypt|decrypt|why|check|gen|scan|guard|reveal|project|"project ls"|"project new"|"project rename"|"project rm"|env|"env ls"|"env new"|"env rename"|"env copy"|"env rm"|machine|"machine enroll"|upgrade|completions|hook|schema|lsp|help) cmd="${cmd:+$cmd }$word" ;;
            *) break ;;
        esac
    done

    case "$prev" in
        --format) COMPREPLY=($(compgen -W "json text" -- "$cur")); return ;;
        --provider) COMPREPLY=(); return ;;
        --guards) COMPREPLY=(); return ;;
        --output) COMPREPLY=(); return ;;
        --env) COMPREPLY=(); return ;;
        --org) COMPREPLY=(); return ;;
        --value) COMPREPLY=(); return ;;
        --out) COMPREPLY=(); return ;;
        --approval) COMPREPLY=(); return ;;
    esac

    words=""
    system=""
    case "$cmd" in
        "") words="init run push pull login logout set unset ls bundle encrypt decrypt why check gen scan guard reveal project env machine upgrade completions hook schema lsp help --json --format --agent --provider" ;;
        init) words="--force --guards --no-guards --output --json --format --agent --provider" ;;
        run) words="--env --no-mask --no-preload --sealed --json --format --agent --provider"; system=-c ;;
        push) words="--env --org --prune --json --format --agent --provider" ;;
        pull) words="--env --i-am-human --json --format --agent --provider" ;;
        login) words="--json --format --agent --provider" ;;
        logout) words="--json --format --agent --provider" ;;
        set) words="--env --value --json --format --agent --provider" ;;
        unset) words="--env --json --format --agent --provider" ;;
        ls) words="--env --json --format --agent --provider" ;;
        bundle) words="--env --json --format --agent --provider" ;;
        encrypt) words="--json --format --agent --provider" ;;
        decrypt) words="--json --format --agent --provider" ;;
        why) words="--env --json --format --agent --provider" ;;
        check) words="--env --strict --json --format --agent --provider" ;;
        gen) words="--out --check --options --json --format --agent --provider" ;;
        scan) words="--staged --install-hook --env --json --format --agent --provider" ;;
        guard) words="--all --check --json --format --agent --provider" ;;
        reveal) words="--env --approval --json --format --agent --provider" ;;
        project) words="ls new rename rm --json --format --agent --provider" ;;
        "project ls") words="--org --json --format --agent --provider" ;;
        "project new") words="--org --json --format --agent --provider" ;;
        "project rename") words="--org --json --format --agent --provider" ;;
        "project rm") words="--org --json --format --agent --provider" ;;
        env) words="ls new rename copy rm --json --format --agent --provider" ;;
        "env ls") words="--json --format --agent --provider" ;;
        "env new") words="--json --format --agent --provider" ;;
        "env rename") words="--json --format --agent --provider" ;;
        "env copy") words="--json --format --agent --provider" ;;
        "env rm") words="--json --format --agent --provider" ;;
        machine) words="enroll --json --format --agent --provider" ;;
        "machine enroll") words="--json --format --agent --provider" ;;
        upgrade) words="--check --json --format --agent --provider" ;;
        completions) words="bash zsh fish powershell elvish --json --format --agent --provider" ;;
        hook) words="--json --format --agent --provider" ;;
        schema) words="--json --format --agent --provider" ;;
        lsp) words="--json --format --agent --provider" ;;
        help) words="--json --format --agent --provider" ;;
    esac
    COMPREPLY=($(compgen -W "$words" $system -- "$cur"))
}
complete -o default -F _penv penv
