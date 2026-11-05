# desl phase 0: build and install the two throwaway prototypes for hands-on testing.
#
#   make install     release build, copy desl-proto, proto-a and proto-b to ~/.local/bin
#   make uninstall   remove them again (stops running hosts; keeps the logs)
#   make purge       uninstall and delete the hosts' logs, the launcher log and the sockets
#   make test        cargo test
#   make build       release build only
#
# BINDIR defaults to ~/.local/bin; override with `make install BINDIR=/some/dir`.

BINDIR ?= $(HOME)/.local/bin
MANIFEST := phase0/Cargo.toml
TARGET := phase0/target/release
BINS := proto-a proto-b desl-proto
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
		echo "stopping running desl hosts (A's logs are kept and replay on demand; B's agents end)"; \
		pkill -f 'proto-[ab] --host' || true; sleep 0.5; \
	fi

install: build stop-hosts
	@mkdir -p "$(BINDIR)"
	@for b in $(BINS); do install -m 755 "$(TARGET)/$$b" "$(BINDIR)/$$b" && echo "installed $(BINDIR)/$$b"; done
	@case ":$$PATH:" in *":$(BINDIR):"*) ;; *) echo "note: $(BINDIR) is not on your PATH";; esac
	@echo
	@echo "Run inside a git repository you trust:"
	@echo "  desl-proto       blind launcher: picks A or B for the session and logs which (desl-proto log)"
	@echo "  proto-a          rendered prototype; /quit or ctrl+b d detaches; proto-a stop stops its host"
	@echo "  proto-b          native prototype (the agents' own TUIs); ctrl+b ? for keys; ctrl+b d detaches; proto-b stop"
	@echo "All need claude and codex on PATH and logged in. State: $(STATE_BASE)/desl-proto*"

uninstall: stop-hosts
	@for b in $(BINS); do rm -f "$(BINDIR)/$$b" && echo "removed $(BINDIR)/$$b"; done
	@echo "Logs kept in $(STATE_BASE)/desl-proto*; \`make purge\` deletes them."

purge: uninstall
	@rm -rf $(STATE_DIRS) $(RUNTIME_DIRS) && echo "deleted $(STATE_DIRS) $(RUNTIME_DIRS)"
