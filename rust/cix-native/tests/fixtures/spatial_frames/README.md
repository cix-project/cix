# Historical spatial-frame fixtures

These four tiny, distribution-local fixtures are byte-for-byte copies of the
frozen `dev-images-1` qualification members. They are retained here so the
crate tests do not depend on research documentation paths.

| file | SHA-256 | source |
| --- | --- | --- |
| `jls.cix` | `5b20d6a1dccc9c2fb029c2bc329be04dbba68fd9fb43c12375d739fa0b0fece2` | `qualification/dev-images-1/01-spatial-jls.cix` |
| `jls-input.bin` | `810bdbd886b9643ef36ebe2199847c6324bd8ed56688c316f5a02cceb63b8f1f3` | `qualification/dev-images-1/01-u8range.bin` |
| `j2k.cix` | `2fc2a31dd5337774953444ca4f50c93317719b104139566ee0695cb5a2ae2563` | `qualification/dev-images-1/02-spatial-j2k.cix` |
| `j2k-input.bin` | `6c91c4e0f391b8171ada33ec59f8bd1a51ab2d295c9c85b98f4aa8a093235eb5` | `qualification/dev-images-1/02-highbit.bin` |

The vectors define a bounded historical spatial-frame compatibility contract.
Their exact checked-in names and SHA-256 identities above are sufficient for
public source tests; no generator, reference runtime, or external data is
required.
