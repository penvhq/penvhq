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
            --format|--guards|--output|--env|--org|--value|--out|--approval) skip=1; continue ;;
            -*) continue ;;
        esac
        case "${cmd:+$cmd }$word" in
            init|run|push|pull|login|logout|set|unset|ls|check|gen|scan|guard|reveal|project|"project ls"|"project new"|"project rename"|"project rm"|env|"env ls"|"env new"|"env rename"|"env copy"|"env rm"|machine|"machine enroll"|upgrade|completions|hook|schema|help) cmd="${cmd:+$cmd }$word" ;;
            *) break ;;
        esac
    done

    case "$prev" in
        --format) COMPREPLY=($(compgen -W "json text" -- "$cur")); return ;;
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
        "") words="init run push pull login logout set unset ls check gen scan guard reveal project env machine upgrade completions hook schema help --json --format --agent" ;;
        init) words="--force --guards --no-guards --output --json --format --agent" ;;
        run) words="--env --no-mask --no-preload --json --format --agent"; system=-c ;;
        push) words="--env --org --prune --json --format --agent" ;;
        pull) words="--env --i-am-human --json --format --agent" ;;
        login) words="--json --format --agent" ;;
        logout) words="--json --format --agent" ;;
        set) words="--env --value --json --format --agent" ;;
        unset) words="--env --json --format --agent" ;;
        ls) words="--env --json --format --agent" ;;
        check) words="--env --json --format --agent" ;;
        gen) words="--out --check --options --json --format --agent" ;;
        scan) words="--staged --install-hook --env --json --format --agent" ;;
        guard) words="--all --check --json --format --agent" ;;
        reveal) words="--env --approval --json --format --agent" ;;
        project) words="ls new rename rm --json --format --agent" ;;
        "project ls") words="--org --json --format --agent" ;;
        "project new") words="--org --json --format --agent" ;;
        "project rename") words="--org --json --format --agent" ;;
        "project rm") words="--org --json --format --agent" ;;
        env) words="ls new rename copy rm --json --format --agent" ;;
        "env ls") words="--json --format --agent" ;;
        "env new") words="--json --format --agent" ;;
        "env rename") words="--json --format --agent" ;;
        "env copy") words="--json --format --agent" ;;
        "env rm") words="--json --format --agent" ;;
        machine) words="enroll --json --format --agent" ;;
        "machine enroll") words="--json --format --agent" ;;
        upgrade) words="--check --json --format --agent" ;;
        completions) words="bash zsh fish powershell elvish --json --format --agent" ;;
        hook) words="--json --format --agent" ;;
        schema) words="--json --format --agent" ;;
        help) words="--json --format --agent" ;;
    esac
    COMPREPLY=($(compgen -W "$words" $system -- "$cur"))
}
complete -o default -F _penv penv
