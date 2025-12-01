---
description: Regenerate WAL code from FlatBuffers schema
---
To regenerate the Rust code for the Write-Ahead Log (WAL) from the `wal.fbs` schema:

1.  Ensure you have `flatc` installed.
2.  Run the following command in the project root:

```bash
flatc --rust -o src wal.fbs
```

This will generate `src/wal_generated.rs`.

**Note:** The build process (`build.rs`) also automatically generates this file into the `OUT_DIR` during `cargo build`. The manual generation is only needed if you want to inspect the generated code or if you are not using `cargo` to build.
