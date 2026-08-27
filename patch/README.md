Reserved for a carried driver patch.

Empty by design. The overlay binds to the existing `hynitron,cst340`
compatible, so no driver source change is required unless probe fails the
identity check at 0xD1FC with `ic mismatch, chkcode is ...`.

If that happens, add a `hynitron,cst328` entry to
`drivers/input/touchscreen/hynitron_cstxxx.c`, keep the modified source in
`source_files/<ver>/touchscreen/`, and generate the patch here. `apply_custom()`
in download_build.sh then needs a step to apply it.
