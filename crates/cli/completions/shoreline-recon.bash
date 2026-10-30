_shoreline__recon() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="shoreline__recon"
                ;;
            shoreline__recon,completions)
                cmd="shoreline__recon__subcmd__completions"
                ;;
            shoreline__recon,config)
                cmd="shoreline__recon__subcmd__config"
                ;;
            shoreline__recon,help)
                cmd="shoreline__recon__subcmd__help"
                ;;
            shoreline__recon,plan)
                cmd="shoreline__recon__subcmd__plan"
                ;;
            shoreline__recon,run)
                cmd="shoreline__recon__subcmd__run"
                ;;
            shoreline__recon,tools)
                cmd="shoreline__recon__subcmd__tools"
                ;;
            shoreline__recon__subcmd__help,completions)
                cmd="shoreline__recon__subcmd__help__subcmd__completions"
                ;;
            shoreline__recon__subcmd__help,config)
                cmd="shoreline__recon__subcmd__help__subcmd__config"
                ;;
            shoreline__recon__subcmd__help,help)
                cmd="shoreline__recon__subcmd__help__subcmd__help"
                ;;
            shoreline__recon__subcmd__help,plan)
                cmd="shoreline__recon__subcmd__help__subcmd__plan"
                ;;
            shoreline__recon__subcmd__help,run)
                cmd="shoreline__recon__subcmd__help__subcmd__run"
                ;;
            shoreline__recon__subcmd__help,tools)
                cmd="shoreline__recon__subcmd__help__subcmd__tools"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        shoreline__recon)
            opts="-o -h -V --tools --work --out --name --fps --steps --matcher --videos --config --help --version run config plan tools completions help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --tools)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --work)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -o)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --fps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --steps)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --matcher)
                    COMPREPLY=($(compgen -W "sequential exhaustive" -- "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__completions)
            opts="-h --tools --work --help bash elvish fish powershell zsh"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --tools)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --work)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__config)
            opts="-h --tools --work --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --tools)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --work)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__help)
            opts="run config plan tools completions help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__help__subcmd__completions)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__help__subcmd__config)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__help__subcmd__plan)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__help__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__help__subcmd__tools)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__plan)
            opts="-h --tools --work --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --tools)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --work)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__run)
            opts="-h --stage --tools --work --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --stage)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --tools)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --work)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        shoreline__subcmd__recon__subcmd__tools)
            opts="-h --tools --work --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --tools)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --work)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _shoreline__recon -o nosort -o bashdefault -o default shoreline-recon
else
    complete -F _shoreline__recon -o bashdefault -o default shoreline-recon
fi
