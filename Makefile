# desl: build, check and install.
#
#   make install     release build, copy desl to ~/.local/bin (stops a running daemon first)
#   make uninstall   stop the daemon and remove desl; threads are kept
#   make check       clippy with warnings denied, formatting, every test
#   make test        cargo test
#   make build       release build only
#
# BINDIR defaults to ~/.local/bin; override with `make install BINDIR=/some/dir`.

BINDIR ?= $(HOME)/.local/bin
TARGET := target/release

.PHONY: help build test check install uninstall

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

# A daemon from an older build would keep serving the old code, so installing stops it. Its
# agents end; every thread is kept and the native sessions resume on the next prompt.
install: build
	@mkdir -p "$(BINDIR)"
	@if "$(TARGET)/desl" status --json 2>/dev/null | grep -q '"running":true'; then \
		echo "stopping the running desl daemon (threads are kept; sessions resume on the next prompt)"; \
		"$(TARGET)/desl" daemon stop >/dev/null || true; \
	fi
	@install -m 755 "$(TARGET)/desl" "$(BINDIR)/desl" && echo "installed $(BINDIR)/desl"
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
