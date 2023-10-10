#!/usr/bin/env bash
# Function to verify the signature of the last commit
verify_signature() {
	local commit_hash
  commit_hash=$(git rev-parse HEAD)
	if ! git verify-commit "$commit_hash"; then
		echo "Error: Last commit ($commit_hash) is not signed."
		exit 1
	fi
}

# Verify the signature of the last commit
verify_signature

# Allow the push to proceed if the signature is valid
exit 0
