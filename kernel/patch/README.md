Reserved for a carried driver patch.

Empty. Not because none is needed, but because what it should contain is not
yet established.

## The measured failure

The module binds to the CST328 and probe fails:

    Hynitron-TS 1-001a: cst3xx_bootloader_enter unable to enter bootloader mode

Not the identity check at `0xD1FC` that this file previously anticipated. The
driver never reaches that test: `cst3xx_bootloader_enter` fails first, and
everything after it is unreachable.

So adding a `hynitron,cst328` compatible string on its own would achieve
nothing. Binding is not the problem; the bootloader handshake is.

## What needs establishing first

Read the CST328 datasheet section on bootloader mode against
`cst3xx_bootloader_enter` in `drivers/input/touchscreen/hynitron_cstxxx.c` and
determine which of these it is:

- the CST328 uses a different entry sequence, in which case the driver needs a
  per-part sequence selected by compatible string
- the CST328 needs no bootloader entry at all, in which case the firmware
  update path should be skipped for this part and probe should proceed
  straight to the identity check

Guessing between those two and writing a patch for the wrong one wastes a
kernel build cycle, which is nearly two hours in CI.

## When there is a patch

Add the driver change, keep the modified source in
`source_files/<ver>/touchscreen/`, and generate the patch here.
`apply_custom()` in `download_build.sh` then needs a step to apply it, and the
overlay's `compatible` may need changing from `hynitron,cst340` depending on
which of the two shapes above applies.

## Priority

Low. The userspace renderer in `runtime/` reads the CST328 over i2cdev, has
working touch, and needs none of this. A kernel input device would only matter
for something that consumes `/dev/input/event*` directly, such as X or a
different display stack.
