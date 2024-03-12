#!/usr/bin/env bash
# Function to verify the signature of the last commit
verify_signature() {
    local commit_hash
    commit_hash=$(git rev-parse HEAD)
    if ! git verify-commit "$commit_hash"; then
        # Allow ssh sign, because the ci server does not set gpg.ssh.allowedSignersFile, so there will be an alert
        if ! git verify-commit "$commit_hash" 2>&1 | grep 'gpg.ssh.allowedSignersFile needs to be configured'; then
            echo "Error: Last commit ($commit_hash) is not signed."
            exit 1
        fi
    fi
}

# Verify the signature of the last commit
verify_signature

# Allow the push to proceed if the signature is valid
exit 0
