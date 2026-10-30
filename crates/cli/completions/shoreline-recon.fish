# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_shoreline_recon_global_optspecs
    string join \n tools= work= o/out= name= fps= steps= matcher= videos config= h/help V/version
end

function __fish_shoreline_recon_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_shoreline_recon_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_shoreline_recon_using_subcommand
    set -l cmd (__fish_shoreline_recon_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l tools -d 'Host tools file; its tables replace those of every other tools layer' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l work -d 'Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/. Default: ./work for subcommands, the user cache directory for a video' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -s o -l out -d 'Output directory for the deliverables. Default: ./<name>-recon' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l name -d 'Dataset name (cache directory and report). Default: the video\'s file stem' -r
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l fps -d 'Frames extracted per second of video (frames.fps)' -r
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l steps -d 'Splat training steps (splat.steps)' -r
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l matcher -d 'Feature matcher (sfm.matcher)' -r -f -a "sequential\t''
exhaustive\t''"
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l config -d 'Partial config (any keys of defaults.toml), merged over the defaults' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -l videos -d 'Also render the showcase and processing-step videos (export.videos)'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -s V -l version -d 'Print version'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -a "run" -d 'Run the implemented stages for a dataset config. Relative paths in the config are resolved against the working directory'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -a "config" -d 'Print the resolved config: built-in defaults, the user\'s config.toml, defaults.toml beside the config, the config. Without a config, the defaults with a placeholder dataset'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -a "plan" -d 'Print the stage execution order'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -a "tools" -d 'Print the resolved tools: built-in, the user\'s tools.toml, tools.toml beside the config, then --tools'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -a "completions" -d 'Print the completion script for a shell, e.g. `eval "$(shoreline-recon completions zsh)"` in ~/.zshrc'
complete -c shoreline-recon -n "__fish_shoreline_recon_needs_command" -a "help" -d 'Print this message or the help of the given subcommand(s)'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand run" -l stage -d 'Run only this stage and what it depends on (repeatable)' -r
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand run" -l tools -d 'Host tools file; its tables replace those of every other tools layer' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand run" -l work -d 'Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/. Default: ./work for subcommands, the user cache directory for a video' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand run" -s h -l help -d 'Print help'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand config" -l tools -d 'Host tools file; its tables replace those of every other tools layer' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand config" -l work -d 'Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/. Default: ./work for subcommands, the user cache directory for a video' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand config" -s h -l help -d 'Print help'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand plan" -l tools -d 'Host tools file; its tables replace those of every other tools layer' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand plan" -l work -d 'Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/. Default: ./work for subcommands, the user cache directory for a video' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand plan" -s h -l help -d 'Print help'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand tools" -l tools -d 'Host tools file; its tables replace those of every other tools layer' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand tools" -l work -d 'Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/. Default: ./work for subcommands, the user cache directory for a video' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand tools" -s h -l help -d 'Print help'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand completions" -l tools -d 'Host tools file; its tables replace those of every other tools layer' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand completions" -l work -d 'Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/. Default: ./work for subcommands, the user cache directory for a video' -r -F
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand completions" -s h -l help -d 'Print help'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand help; and not __fish_seen_subcommand_from run config plan tools completions help" -f -a "run" -d 'Run the implemented stages for a dataset config. Relative paths in the config are resolved against the working directory'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand help; and not __fish_seen_subcommand_from run config plan tools completions help" -f -a "config" -d 'Print the resolved config: built-in defaults, the user\'s config.toml, defaults.toml beside the config, the config. Without a config, the defaults with a placeholder dataset'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand help; and not __fish_seen_subcommand_from run config plan tools completions help" -f -a "plan" -d 'Print the stage execution order'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand help; and not __fish_seen_subcommand_from run config plan tools completions help" -f -a "tools" -d 'Print the resolved tools: built-in, the user\'s tools.toml, tools.toml beside the config, then --tools'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand help; and not __fish_seen_subcommand_from run config plan tools completions help" -f -a "completions" -d 'Print the completion script for a shell, e.g. `eval "$(shoreline-recon completions zsh)"` in ~/.zshrc'
complete -c shoreline-recon -n "__fish_shoreline_recon_using_subcommand help; and not __fish_seen_subcommand_from run config plan tools completions help" -f -a "help" -d 'Print this message or the help of the given subcommand(s)'
