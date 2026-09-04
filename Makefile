# Top-level dispatch. The two halves of this repository are independent and
# have nothing in common but the hardware they target, so this file delegates
# rather than sharing anything between them.
#
#   kernel/   cross-compiles a kernel module and a device tree overlay
#   runtime/  cross-compiles a userspace Rust binary
#
# backend=spi cannot share spi0 cs0 with an fbtft overlay. backend=framebuffer
# can: it draws into the fb_st7789v node while fbtft holds the bus. Building both halves
# is always fine.

.PHONY: help kernel runtime validate fmt-fix dist clean

help:
	@echo "targets:"
	@echo "  validate   fmt, clippy and tests for the runtime, no build"
	@echo "  fmt-fix    apply rustfmt to the runtime"
	@echo "  runtime    validate then cross-build the userspace renderer"
	@echo "  dist       runtime plus packaged binaries and checksums"
	@echo "  kernel     build the CST328 module and overlay (see kernel/README.md)"
	@echo "  clean      clean both"
	@echo ""
	@echo "each subtree can also be built directly:"
	@echo "  cd kernel/build-script && ./download_build.sh"
	@echo "  cd runtime && make"

# Fast feedback without waiting on three cross builds.
validate:
	$(MAKE) -C runtime validate

fmt-fix:
	$(MAKE) -C runtime fmt-fix

runtime:
	$(MAKE) -C runtime

dist:
	$(MAKE) -C runtime dist

kernel:
	cd kernel/build-script && ./download_build.sh

clean:
	$(MAKE) -C runtime clean
	rm -rf kernel/linux-*/
