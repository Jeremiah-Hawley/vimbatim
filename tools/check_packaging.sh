#!/usr/bin/env bash
set -euo pipefail

workflow=.github/workflows/beta-build.yml
grep -Fq 'cp default_settings.conf dist/Vimbatim-Windows/' "$workflow"
! grep -Fq 'cp settings.conf dist/Vimbatim-Windows/' "$workflow"

# Icon Packaging Checks
# Ensure all icons are tracked
for svg in icons/*.svg; do
    if ! git ls-files --error-unmatch "$svg" >/dev/null 2>&1; then
        echo "Error: $svg is not tracked by Git."
        exit 1
    fi
done

# Ensure the binary embeds the assets
if ! grep -Fq '.with_assets(' src/lib.rs; then
    echo "Error: Application is not built with VimbatimAssets."
    exit 1
fi

# Ensure icons/ is NOT copied in the CI distribution by asserting it doesn't appear in the cp command
if grep -Fq 'cp -r icons' "$workflow"; then
    echo "Error: icons directory is copied into the distribution but it should be embedded."
    exit 1
fi

