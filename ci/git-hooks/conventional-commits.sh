#!/usr/bin/env bash

# Conventional commit regex pattern
# bash regex do not support \w \d \s etc
pattern="^(build|ci|docs|feat|fix|perf|refactor|style|test|chore|revert)(\([[:alnum:]\-]+\))?:[[:space:]].*"

# Read the commit message from the file
commit_file="$1"
commit_head_message=$(head -n1 "$commit_file")

# Check if the commit message matches the pattern
if ! [[ "$commit_head_message" =~ $pattern ]]; then
	echo "Error: Invalid commit message format. Please use a conventional commit message."
	echo "Commit message should match the pattern: $pattern."
	echo "Please refer to https://www.conventionalcommits.org/en/v1.0.0/ for more details."
	exit 1
fi

# If the commit message matches the pattern, the hook exits successfully
exit 0
