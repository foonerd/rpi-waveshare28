# waveshare28-panel cross builds.
#
# Default target set is musl, statically linked, runs on Buster through Trixie.
# The gnu set is provided for parity with sister repos; those binaries need
# glibc 2.31 or newer and will not run on Volumio 3.
#
# Every build target depends on `validate` first. Cross-compiling three targets
# to discover a type error on the third wastes minutes on something cargo check
# reports in seconds, and a warning found after the artefacts exist tends not
# to get fixed.

CRATE      := waveshare28-panel
MUSL       := arm-unknown-linux-musleabihf armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl
GNU        := armv7-unknown-linux-gnueabihf aarch64-unknown-linux-gnu
DIST       := dist

.PHONY: all validate check fmt fmt-fix clippy test musl gnu dist clean

all: musl

# Everything the CI lint job runs, in the order that fails cheapest first.
# Formatting is a parse, clippy is a full check, tests actually execute.
validate: fmt clippy test

check:
	cargo check --all-targets

fmt:
	cargo fmt --all -- --check

fmt-fix:
	cargo fmt --all

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

musl: validate
	@for t in $(MUSL); do \
		echo "=== $$t"; \
		cross build --release --target $$t || exit 1; \
	done

gnu: validate
	@for t in $(GNU); do \
		echo "=== $$t"; \
		cross build --release --target $$t || exit 1; \
	done

dist: musl
	@mkdir -p $(DIST)
	@for t in $(MUSL); do \
		cp target/$$t/release/$(CRATE) $(DIST)/$(CRATE)-$$t; \
	done
	@cd $(DIST) && sha256sum $(CRATE)-* > SHA256SUMS
	@ls -l $(DIST)

clean:
	cargo clean
	rm -rf $(DIST)
