#!/bin/sh
# GNU tar/libarchive external program filter for an installed CIX binary.
set -eu

memory=${CIX_TAR_MEMORY:-512MiB}
if [ "${1-}" = '--cix' ]; then
  if [ "$#" -lt 2 ]; then
    printf '%s\n' 'cix-tar-filter: --cix requires an absolute executable path' >&2
    exit 64
  fi
  cix_bin=$2
  shift 2
elif [ -n "${CIX_TAR_CIX:-}" ]; then
  cix_bin=$CIX_TAR_CIX
else
  # libarchive 3.7.2 spawns program filters with an empty environment.  Its
  # public program API supplies this script as an absolute argv[0], so a CIX
  # binary installed beside it is a deterministic, PATH-free fallback.
  case "$0" in
    /*)
      script_dir=${0%/*}
      test -n "$script_dir" || script_dir=/
      cix_bin=$script_dir/cix
      ;;
    *)
      printf '%s\n' 'cix-tar-filter: require --cix ABS or CIX_TAR_CIX' >&2
      exit 64
      ;;
  esac
fi
if [ "$#" -gt 1 ]; then
  printf '%s\n' 'cix-tar-filter: expected at most one mode argument' >&2
  exit 64
fi
case "$cix_bin" in
  /*) ;;
  *) printf '%s\n' 'cix-tar-filter: CIX_TAR_CIX must be an absolute path' >&2; exit 64 ;;
esac
if [ ! -x "$cix_bin" ]; then
  printf '%s\n' 'cix-tar-filter: CIX_TAR_CIX is not executable' >&2
  exit 66
fi
case "${1-}" in
  '') exec "$cix_bin" --stream --memory "$memory" -c ;;
  -d|--decode) exec "$cix_bin" -d --stream --memory "$memory" -c ;;
  --encode) exec "$cix_bin" --stream --memory "$memory" -c ;;
  *)
    printf '%s\n' 'cix-tar-filter: expected no argument, -d, --encode, or --decode' >&2
    exit 64
    ;;
esac
