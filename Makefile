# desl phase 0: build and install Prototype A, the phase 0 winner, for hands-on use.
#
#   make install     release build, copy proto-a to ~/.local/bin
#   make uninstall   remove it again (stops running hosts; keeps the logs)
#   make purge       uninstall and delete the hosts' logs and the sockets
#   make test        cargo test
#   make build       release build only
#
# BINDIR defaults to ~/.local/bin; override with `make install BINDIR=/some/dir`.

BINDIR ?= $(HOME)/.local/bin
MANIFEST := phase0/Cargo.toml
TARGET := phase0/target/release
BINS := proto-a
# Installed by earlier builds. Prototype B and the blind launcher were removed when A won;
# install and uninstall clear them out of BINDIR so the old commands do not linger.
RETIRED := proto-b desl-proto
STATE_BASE := $(or $(XDG_STATE_HOME),$(HOME)/.local/state)
RUNTIME_BASE := $(or $(XDG_RUNTIME_DIR),/tmp)
STATE_DIRS := $(STATE_BASE)/desl-proto-a $(STATE_BASE)/desl-proto-b $(STATE_BASE)/desl-proto
RUNTIME_DIRS := $(RUNTIME_BASE)/desl-proto-a $(RUNTIME_BASE)/desl-proto-b

.PHONY: help build test install uninstall purge stop-hosts

help:
	@sed -n '2,10p' $(MAKEFILE_LIST) | sed 's/^# \{0,1\}//'

build:
	cargo build --release --manifest-path $(MANIFEST)

test:
	cargo test --manifest-path $(MANIFEST)

# Hosts keep running after the client leaves. A host from an older build would keep serving an
# older protocol, so installing stops them; they restart on demand and replay their logs.
# The bracket keeps the pattern from matching this recipe's own shell.
stop-hosts:
	@if pgrep -f 'proto-[ab] --host' >/dev/null 2>&1; then \
		echo "stopping running desl hosts (their logs are kept and replay on demand)"; \
		pkill -f 'proto-[ab] --host' || true; sleep 0.5; \
	fi

install: build stop-hosts
	@mkdir -p "$(BINDIR)"
	@for b in $(BINS); do install -m 755 "$(TARGET)/$$b" "$(BINDIR)/$$b" && echo "installed $(BINDIR)/$$b"; done
	@for b in $(RETIRED); do if [ -e "$(BINDIR)/$$b" ]; then rm -f "$(BINDIR)/$$b" && echo "removed $(BINDIR)/$$b (retired with Prototype B)"; fi; done
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "note: $(BINDIR) is not on your PATH";; esac
	@echo
	@echo "Run inside a git repository you trust:"
	@echo "  proto-a          attach; ? lists the keys; /quit or ctrl+b d detaches and the agents keep running"
	@echo "  proto-a status   the sidebar as text"
	@echo "  proto-a stop     stop the project's host; its log is kept"
	@echo "Needs claude and codex on PATH and logged in. State: $(STATE_BASE)/desl-proto-a"

uninstall: stop-hosts
	@for b in $(BINS) $(RETIRED); do rm -f "$(BINDIR)/$$b" && echo "removed $(BINDIR)/$$b"; done
	@echo "Logs kept in $(STATE_BASE)/desl-proto*; \`make purge\` deletes them."

purge: uninstall
	@rm -rf $(STATE_DIRS) $(RUNTIME_DIRS) && echo "deleted $(STATE_DIRS) $(RUNTIME_DIRS)"
