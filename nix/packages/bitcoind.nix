{ lib
, stdenv
, fetchFromGitHub
, autoreconfHook
, pkg-config
, installShellFiles
, util-linux
, hexdump
, boost
, libevent
, libtool
, miniupnpc
, zeromq
, zlib
, db48
, sqlite
, python3
, withWallet ? true
}:
stdenv.mkDerivation rec {
  pname = "bitcoind";
  version = "25.x";

  src = fetchFromGitHub {
    owner = "bitcoin";
    repo = "bitcoin";
    rev = "8087626cbda97f6def4fb51cdac6f204486ee6fb";
    hash = "sha256-FV54PRKHIUIGlavsItnx5zo/k++oeQNPsbiLahrkOqU=";
  };

  nativeBuildInputs =
    [ autoreconfHook pkg-config installShellFiles libtool ]
    ++ lib.optionals stdenv.isLinux [ util-linux ]
    ++ lib.optionals stdenv.isDarwin [ hexdump ];

  buildInputs = [ boost libevent miniupnpc zeromq zlib ]
    ++ lib.optionals withWallet [ db48 sqlite ];

  preConfigure = lib.optionalString stdenv.isDarwin ''
    export MACOSX_DEPLOYMENT_TARGET=10.13
  '';

  configureFlags = [
    "--with-boost-libdir=${boost.out}/lib"
    "--with-gui=no"
    "--disable-bench"
    "--disable-tests"
    "--disable-gui-tests"
  ];

  nativeCheckInputs = [ python3 ];

  doCheck = false;

  checkFlags =
    [ "LC_ALL=en_US.UTF-8" ];

  enableParallelBuilding = true;

  meta = with lib; {
    description = "Peer-to-peer electronic cash system";
    longDescription = ''
      Bitcoin is a free open source peer-to-peer electronic cash system that is
      completely decentralized, without the need for a central server or trusted
      parties. Users hold the crypto keys to their own money and transact directly
      with each other, with the help of a P2P network to check for double-spending.
    '';
    homepage = "https://bitcoin.org/en/";
    downloadPage = "https://bitcoincore.org/bin/bitcoin-core-${version}/";
    changelog = "https://bitcoincore.org/en/releases/${version}/";
    maintainers = with maintainers; [ prusnak roconnor ];
    license = licenses.mit;
    platforms = platforms.unix;
  };
}
