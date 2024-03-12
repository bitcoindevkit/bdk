{ lib
, stdenv
, rustPlatform
, fetchFromGitHub
, llvmPackages_15
, rocksdb
, Security
}:

let
  # rocksdb = rocksdb_6_23;
in
rustPlatform.buildRustPackage rec {
  pname = "esplora";
  # last tagged version is far behind master
  version = "20230218";

  # fix because of https://github.com/Blockstream/electrs/pull/65
  # src = fetchFromGitHub {
  #   owner = "Blockstream";
  #   repo = "electrs";
  #   rev = "adedee15f1fe460398a7045b292604df2161adc0";
  #   hash = "sha256-KnN5C7wFtDF10yxf+1dqIMUb8Q+UuCz4CMQrUFAChuA=";
  # };
  src = fetchFromGitHub {
    owner = "Nsandomeno";
    repo = "electrs";
    rev = "b9c7b5a052254a1096d7641803254fdc0b65efb5";
    hash = "sha256-2yKaV+cY4/1KqOKgZce18EG323OBsGwVy9tQrtN4scw=";
  };
  cargoLock = {
    lockFile = ./Cargo.lock;
    allowBuiltinFetchGit = true;
  };


  # needed for librocksdb-sys
  nativeBuildInputs = [ rustPlatform.bindgenHook ];

  # link rocksdb dynamically
  ROCKSDB_INCLUDE_DIR = "${rocksdb}/include";
  ROCKSDB_LIB_DIR = "${rocksdb}/lib";

  buildInputs = lib.optionals stdenv.isDarwin [ Security ];

  # rename to avoid a name conflict with other electrs package
  postInstall = ''
    mv $out/bin/electrs $out/bin/esplora
  '';

  meta = with lib; {
    description = "Blockstream's re-implementation of Electrum Server for Esplora";
    homepage = "https://github.com/Blockstream/electrs";
    license = licenses.mit;
    maintainers = with maintainers; [ jkitman ];
  };
}
