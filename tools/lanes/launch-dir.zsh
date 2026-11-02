# Sourced by chain-ensemble.sh and ensemble-read.sh (#416): one fresh output directory per launch.
#
#   launch_dir <given directory or empty> <prefix>   sets $REPLY to the launch's directory
#
# Without a given directory, a new directory under $TMPDIR. A given directory that does not exist
# yet, or exists and is empty, is the launch's own. Any other given directory gets a new
# timestamped subfolder (<UTC time>-XXXXXX) per launch, so a second launch into one directory
# never overwrites an earlier launch's outputs (2026-11-01: a second chain launch into one OUT_DIR
# overwrote lane.txt). A directory is claimed by writing its launch.txt with noclobber, so two
# launches racing for one empty directory cannot both take it: the loser gets a subfolder.
launch_dir() {
  emulate -L zsh
  setopt no_clobber
  zmodload zsh/datetime
  local given=$1 prefix=$2 ts
  local -a held
  TZ=UTC strftime -s ts '%Y%m%dT%H%M%SZ' $EPOCHSECONDS
  if [[ -z $given ]]; then
    REPLY=$(mktemp -d "${TMPDIR:-/Volumes/Lanes/targets}/$prefix.XXXXXX") || return 1
  else
    mkdir -p -- $given || return 1
    held=($given/*(DN))
    if (( ! ${#held} )) && { print -r -- "$prefix launched $ts" > $given/launch.txt } 2>/dev/null; then
      REPLY=$given
      return 0
    fi
    REPLY=$(mktemp -d "$given/$ts-XXXXXX") || return 1
  fi
  print -r -- "$prefix launched $ts" > $REPLY/launch.txt
}
