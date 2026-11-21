# Verify-suite image for the sysknife node suite.
#
# node:24-slim is what CI runs, and it carries no `nc`. packages/setup's
# setup-contract.test.mjs probes a Unix-domain socket with netcat ("a reachable
# externally managed daemon makes a skipped install complete"), so in the bare
# image that test fails unmutated, no receipt is earnable, and the gate reports
# the pull request as failing a test its change never touched. sysknife#749
# (test-only, approved) waited for two review runs on exactly that.
#
# netcat-openbsd is the build Ubuntu CI uses and the one that speaks -U.
#
# git is here too: tests/wizard-harness.mjs runs `git init` in a scratch
# directory, so a test that drives the wizard fails in the bare image for the
# same reason the netcat one did.
#
# Nothing else is added: the package declares no dependencies and the suite
# runs with --network=none.
FROM docker.io/library/node:24-slim
RUN apt-get update -y \
 && apt-get install -y --no-install-recommends git netcat-openbsd \
 && rm -rf /var/lib/apt/lists/*
