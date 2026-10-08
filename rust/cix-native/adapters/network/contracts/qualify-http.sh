#!/bin/sh
set -eu
: "${CIX_NETWORK_HOST:?set only in the scheduled native-host qualification}"
: "${PKG_CONFIG_PATH:?point to the installed cix-native.pc directory}"
exec go test ./contracts -run '^TestControlledPeerHTTPContract$' -count=1
