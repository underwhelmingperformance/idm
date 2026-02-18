_: {
  perSystem = {config, ...}: let
    inherit (config.idm) pkgs rustToolchain rustfmtNightly rustfmtBin platform;
  in {
    devShells.default = pkgs.mkShell {
      buildInputs =
        [
          rustToolchain
          rustfmtNightly
          pkgs.cargo-llvm-cov
          pkgs.just
        ]
        ++ platform.devShellBuildInputs;
      LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath platform.packageRuntimeLibs;
      RUSTFMT = rustfmtBin;
    };
  };
}
