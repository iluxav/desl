# desl: build, check and install.
#
#   make install     release build, copy desl to ~/.local/bin (stops a running daemon first)
#   make uninstall   stop the daemon and remove desl; threads are kept
#   make check       clippy with warnings denied, formatting, every test
#   make test        cargo test
#   make build       release build only
#
#   make proto-install   the phase 0 prototype (proto-a), kept as the interface reference
#   make proto-uninstall
#
# BINDIR defaults to ~/.local/bin; override with `make install BINDIR=/some/dir`.

BINDIR ?= $(HOME)/.local/bin
TARGET := target/release
PROTO_MANIFEST := phase0/Cargo.toml
PROTO_TARGET := phase0/target/release
# Installed by phase 0 builds; removed so the old commands do not linger.
RETIRED := proto-b desl-proto

.PHONY: help build test check install uninstall proto-install proto-uninstall proto-stop-hosts

help:
	@sed -n '2,13p' $(MAKEFILE_LIST) | sed 's/^# \{0,1\}//'

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
	@for b in $(RETIRED); do if [ -e "$(BINDIR)/$$b" ]; then rm -f "$(BINDIR)/$$b" && echo "removed $(BINDIR)/$$b (retired after phase 0)"; fi; done
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "note: $(BINDIR) is not on your PATH";; esac
	@echo
	@echo "Run inside a repository:"
	@echo "  desl             attach; ? lists the keys; /quit or ctrl+b d detaches and the agents keep running"
	@echo "  desl status      the sidebar as text"
	@echo "  desl doctor      check the agents, the daemon and the store"
	@echo "  desl --help      every command"

uninstall:
	@if [ -x "$(BINDIR)/desl" ]; then "$(BINDIR)/desl" daemon stop >/dev/null 2>&1 || true; fi
	@rm -f "$(BINDIR)/desl" && echo "removed $(BINDIR)/desl"
	@echo "Threads are kept in the store; \`desl doctor\` prints where it is."

# ---- the phase 0 prototype, kept as the reference the client is built to match ----

# The bracket keeps the pattern from matching this recipe's own shell.
proto-stop-hosts:
	@if pgrep -f 'proto-[ab] --host' >/dev/null 2>&1; then \
		echo "stopping running prototype hosts (their logs are kept and replay on demand)"; \
		pkill -f 'proto-[ab] --host' || true; sleep 0.5; \
	fi

proto-install: proto-stop-hosts
	cargo build --release --manifest-path $(PROTO_MANIFEST)
	@mkdir -p "$(BINDIR)"
	@install -m 755 "$(PROTO_TARGET)/proto-a" "$(BINDIR)/proto-a" && echo "installed $(BINDIR)/proto-a"

proto-uninstall: proto-stop-hosts
	@rm -f "$(BINDIR)/proto-a" && echo "removed $(BINDIR)/proto-a"
