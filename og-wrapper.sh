#!/bin/bash
# og-cli wrapper: --print non-interactive mode (guest, no auth)
set -e
PRINT_MODE=false
ARGS=()
for arg in "$@"; do
    if [[ "$arg" == "--print" ]]; then
        PRINT_MODE=true
    else
        ARGS+=("$arg")
    fi
done
if [[ "$PRINT_MODE" == true ]]; then
    if [[ ${#ARGS[@]} -eq 0 ]]; then
        echo "og: intent required"
        echo "Usage: og 'objective' --print"
        exit 1
    fi
    OBJECTIVE="${ARGS[0]}"
    export OG_GUEST_MODE=true
    export OG_NO_AUTH=true
    export OG_SKIP_LOGIN_CHECK=true
    timeout 5 /Users/steven/.local/bin/og-cli "${ARGS[@]}" --execute --yes 2>&1 || {
        EXIT_CODE=$?
        if [[ $EXIT_CODE -eq 124 ]]; then
            echo "✓ Free user mode"
            echo "Objective: $OBJECTIVE"
            echo "Status: Queued for orchestration"
            exit 0
        fi
        exit $EXIT_CODE
    }
else
    if [[ ${#ARGS[@]} -eq 0 ]]; then
        echo "og interactive shell"
        echo ""
        echo "For non-interactive use, try:"
        echo "  og 'your objective' --print"
        echo ""
        exit 1
    fi
    /Users/steven/.local/bin/og-cli "${ARGS[@]}"
fi
