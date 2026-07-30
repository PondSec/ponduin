#!/bin/sh
set -eu

unset APPLE_TEAM_ID
unset APPLE_ID
unset APPLE_ID_PASSWORD
unset KEYCHAIN_PATH
unset CSC_LINK
unset CSC_KEY_PASSWORD
unset CSC_NAME
unset CSC_KEYCHAIN

export CSC_IDENTITY_AUTO_DISCOVERY=false
export PONDUIN_DISABLE_KEYRING=1
export PONDUIN_LOCAL_DEV=1

case "${1:-}" in
  --check)
    printf '%s\n' '{"admin_password_required":false,"system_keyring":false,"distribution_signing":false,"applications_write":false}'
    ;;
  --debug)
    exec pnpm run start-gui-debug
    ;;
  --package)
    exec pnpm run package
    ;;
  "")
    exec pnpm run start-gui
    ;;
  *)
    printf 'unknown local development option: %s\n' "$1" >&2
    exit 64
    ;;
esac
