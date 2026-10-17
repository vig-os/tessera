---
type: issue
state: open
created: 2026-10-16T17:10:26Z
updated: 2026-10-16T17:10:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/565
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:49.082Z
---

# [Issue 565]: [build(py): manylinux wheel (auditwheel repair) — the Nix wheel carries a /nix/store RUNPATH and a linux_* tag](https://github.com/vig-os/tessera/issues/565)

## The gap

The Nix-built wheel (`nix build .#wheel`, flake.nix `tessera-wheel`) is a `linux_x86_64` wheel, not a
manylinux one. The flake comment calls the auditwheel/manylinux repair "a deliberate follow-up", but no
issue tracks it (#376/#377 closed without one). Measured on dev fa581f5:

```
$ readelf -d tessera/_native.abi3.so | grep -E 'RUNPATH|NEEDED'
 (RUNPATH) Library runpath: [/nix/store/…-gcc-15.2.0-lib/lib:/nix/store/…-glibc-2.42-61/lib]
 (NEEDED)  libgcc_s.so.1  libm.so.6  libc.so.6  ld-linux-x86-64.so.2
# highest versioned symbol: GLIBC_2.34
```

- The `RUNPATH` points into `/nix/store`. It is dead on a non-Nix host, where the loader falls back to the
  system libraries. That works, but only by accident.
- The `linux_x86_64` tag is rejected by PyPI, so the wheel can't be published there.

## Not this issue: the `LD_LIBRARY_PATH` workaround in #562/#563's repro

A downstream adopter (MorePET/anatomist) needed `LD_LIBRARY_PATH=<nix libstdc++>` to import the wheel from a
plain venv on NixOS. That isn't tessera: `_native.so` doesn't link `libstdc++`. The failing module is
**numpy's PyPI wheel**, which needs a system `libstdc++.so.6` that NixOS doesn't provide:

```
$ python -m venv v && v/bin/pip install numpy <tessera wheel>
$ v/bin/python -c 'import numpy'      # ImportError: libstdc++.so.6: cannot open shared object file
# tessera._native loaded by itself (numpy stubbed): OK
```

On NixOS the fix is a numpy from nixpkgs (`python312.withPackages (p: [p.numpy])`) or `nix-ld`. That belongs
in the install docs.

## Scope

1. Produce a manylinux wheel (auditwheel repair, or a manylinux build container in the release workflow),
   with no `/nix/store` RUNPATH, so it can go to PyPI. Decide whether the Nix `#wheel` output stays
   `linux_*` (reproducible, Nix-only) next to it. That's a distribution-design decision, recorded in
   `docs/book/src/distribution.md`.
2. `docs/book/src/install.md`: a "Python on NixOS" note (numpy from nixpkgs, or nix-ld) so nobody blames
   tessera for numpy's `libstdc++`.
3. A check that fails if the published wheel's RUNPATH contains `/nix/store` or its tag isn't manylinux.

Refs: #376

