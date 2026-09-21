# Preset examples

These files are examples, not defaults. The shell installs nothing into a
harness home and writes no preset of its own (design doc §22); copy what you
want into `~/.dsh/dsh-app/presets/`.

- `second.toml`: a second web preset, useful for running two runtimes at
  once.
- `patch.d/fixed-port.yml`: the patch layer `second.toml` refers to. It pins
  the web server to a fixed port, so the layered `--patch` behaviour is visible.
