Our own minimal Rojo project, built with Rojo 7.7.0 into `code.rbxl`:

```sh
rojo build default.project.json --output code.rbxl
```

The committed `code.rbxl` is the real Rojo output, so scene `include` tests
exercise a native file written by an independent implementation without
requiring Rojo at test time. Rebuild it only when the sources change.
