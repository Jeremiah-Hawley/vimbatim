#!/usr/bin/env bash
set -euo pipefail

workflow=.github/workflows/beta-build.yml
grep -Fq 'cp default_settings.conf dist/Vimbatim-Windows/' "$workflow"
! grep -Fq 'cp settings.conf dist/Vimbatim-Windows/' "$workflow"
