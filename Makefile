# desl: build, check and install.
#
#   make install     release build, copy desl to ~/.local/bin; a running daemon starts again on it and keeps its agents
#   make uninstall   stop the daemon and remove desl; threads are kept
#   make check       clippy with warnings denied, formatting, every test
#   make test        cargo test
#   make build       release build only
#
# BINDIR defaults to ~/.local/bin; override with `make install BINDIR=/some/dir`.

BINDIR ?= $(HOME)/.local/bin
TARGET := target/release

.PHONY: help build test check check-web install uninstall

help:
	@sed -n '2,10p' $(MAKEFILE_LIST) | sed 's/^# \{0,1\}//'

build:
	cargo build --release

test:
	cargo test --workspace

check:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo fmt --all --check
	cargo test --workspace

# Playwright and Chromium are test tools only; the installed binary needs neither.
check-web:
	node --test crates/desl-web/tests/browser.mjs

# The running daemon would keep serving the old code, so it starts again from the binary just
# installed. It hands that one every agent and shell, so nothing that runs in a thread ends and
# an open desl reconnects by itself. A daemon from a build before the handover cannot do that:
# it is restarted the old way when no agent is working, and left running when one is.
install: build
	@mkdir -p "$(BINDIR)"
	@install -m 755 "$(TARGET)/desl" "$(BINDIR)/desl.new" && mv -f "$(BINDIR)/desl.new" "$(BINDIR)/desl" && echo "installed $(BINDIR)/desl"
	@if "$(BINDIR)/desl" status --json 2>/dev/null | grep -q '"running":true'; then \
		"$(BINDIR)/desl" daemon restart || true; \
	fi
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "note: $(BINDIR) is not on your PATH";; esac
	@echo
	@echo "Run inside a repository:"
	@echo "  desl             attach: the agent's own interface; ctrl+space d detaches and the agents keep running"
	@echo "  desl status      the thread list as text"
	@echo "  desl doctor      check the agents, the daemon and the store"
	@echo "  desl --help      every command"

uninstall:
	@if [ -x "$(BINDIR)/desl" ]; then "$(BINDIR)/desl" daemon stop >/dev/null 2>&1 || true; fi
	@rm -f "$(BINDIR)/desl" && echo "removed $(BINDIR)/desl"
	@echo "Threads are kept in the store; \`desl doctor\` prints where it is."
