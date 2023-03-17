## Esplora wallet example with Tor

This example uses [`libtor`](https://github.com/MagicalBitcoin/libtor) by building a Tor binary and using it as a SOCKS5 proxy. 

### Prerequisite

In order to build a Tor binary `libtor` requires a C compiler and C build tools.

Here is how to install them on **Ubuntu**:

```bash
sudo apt install autoconf automake clang file libtool openssl pkg-config
```
 
And here is how to install them on **Mac OS X**:

```bash
xcode-select --install
brew install autoconf
brew install automake
brew install libtool
brew install openssl
brew install pkg-config
export LDFLAGS="-L/opt/homebrew/opt/openssl/lib"
export CPPFLAGS="-I/opt/homebrew/opt/openssl/include"
```

### Running

This example can be run with

```bash
cargo run -p bdk-esplora-wallet-tor-example
```