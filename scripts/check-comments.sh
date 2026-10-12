#!/usr/bin/env bash
trap - EXIT ERR DEBUG RETURN
listing=
trap 'rm -f -- "$listing"' EXIT
set -euo pipefail

builtin unset GIT_LITERAL_PATHSPECS GIT_GLOB_PATHSPECS GIT_NOGLOB_PATHSPECS GIT_ICASE_PATHSPECS \
  GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_NAMESPACE \
  GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES

root=$(command -p git rev-parse --show-toplevel)
rel=$(command -p realpath --relative-to="$root" -- "$0")
builtin cd "$root"

prefixes='(^[[:space:]]*(#|\*|/\*|<!--)|//)'
banned=(
  'TODO'
  'FIXME'
  'XXX'
  'HACK'
  '[Ww]orkaround'
  '[Tt]emporary (fix|hack|solution)'
  '[Qq]uick fix'
  '[Ff]or the time being'
  '[Nn]ot ideal'
  '[Ss]hould be fixed'
  '[Ss]orry'
  '[Kk]ludge'
  '[Bb]and-?aid'
)
joined=$(printf '%s|' "${banned[@]}")
joined=${joined%|}
pattern="${prefixes}.*\\b(${joined})\\b"

if (($#)); then
  pathspec=("$@")
else
  pathspec=(
    '*.rs' '*.qml' '*.js' '*.sh' '*.py' '*.toml' '*.yml'
    ':!:tests/fixtures/**' ':!:**/vendor/**'
  )
fi
# git ls-files rejects a pathspec that leaves the repository.
case $rel in
  ..*) ;;
  *) pathspec+=(":(exclude,literal)$rel") ;;
esac

listing=$(command -p mktemp)
command -p git ls-files -z --recurse-submodules -- "${pathspec[@]}" >"$listing"
files=()
while IFS= builtin read -r -d '' path; do
  if [[ $path == - ]]; then
    files+=("./-")
  else
    files+=("$path")
  fi
done <"$listing"

if ((${#files[@]} == 0)); then
  exit 0
fi

set +e
hits=$(command -p grep -HnE "$pattern" -- "${files[@]}")
status=$?
set -e
if ((status > 1)); then
  if [[ -n $hits ]]; then
    printf '%s\n' "$hits"
  fi
  exit "$status"
fi
if ((status == 0)); then
  printf '%s\n' "$hits"
  printf '\nApologetic or deferred-work comments are not allowed (see AGENTS.md).\n' >&2
  exit 1
fi
