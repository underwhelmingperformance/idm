_: {
  perSystem = {config, ...}: let
    inherit (config.idm) pkgs rustToolchain rustfmtNightly rustfmtBin;
  in {
    devShells.default = pkgs.mkShell {
      buildInputs = [
        rustToolchain
        rustfmtNightly
        pkgs.cargo-llvm-cov
        pkgs.just
      ];
      RUSTFMT = rustfmtBin;
    };
  };
}
