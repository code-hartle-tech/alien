{ lib
, rustPlatform
, pkg-config
, libGL
, libxkbcommon
, wayland
, makeWrapper
}:

rustPlatform.buildRustPackage {
  pname = "alien";
  version = "0.3.1";

  src = lib.cleanSourceWith {
    src = ../..;
    # Keep the closure to what the build needs. research/ in particular must
    # never end up in a store path: it is where decompilation output lands.
    filter = path: type:
      let rel = lib.removePrefix (toString ../.. + "/") (toString path);
      in !(lib.hasPrefix "research" rel
        || lib.hasPrefix "wiki" rel
        || lib.hasPrefix ".git" rel
        || lib.hasPrefix "code/target" rel);
  };

  sourceRoot = "source/code";
  cargoLock.lockFile = ../../code/Cargo.lock;

  nativeBuildInputs = [ pkg-config makeWrapper ];
  buildInputs = [ libGL libxkbcommon wayland ];

  # The unit tests are pure logic and run anywhere. Anything that touches
  # firmware needs root and real hardware, so it is not part of the build.
  doCheck = true;

  postInstall = ''
    # The launcher: GUI -> TUI -> CLI help. Bound to the vendor's
    # PredatorSense key, and the right target for a .desktop action too.
    install -Dm755 tools/alien-launch $out/bin/alien-launch

    install -Dm644 ../packaging/shared/tech.hartle.Alien.desktop \
      $out/share/applications/tech.hartle.Alien.desktop
    install -Dm644 ../packaging/shared/60-alien.rules \
      $out/lib/udev/rules.d/60-alien.rules

    # winit dlopens libwayland-client rather than linking it, so rpath is not
    # enough — without this the GUI exits with NoWaylandLib on a machine where
    # the library is plainly installed. Only the GUI needs it; wrapping the CLI
    # and daemon would pull a GL stack into a headless closure for nothing.
    wrapProgram $out/bin/alien-gui \
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath [ libGL libxkbcommon wayland ]}
  '';

  meta = with lib; {
    description = "Fan, lighting, turbo and telemetry control for Acer Predator/Nitro laptops";
    homepage = "https://alien.hartle.tech";
    license = licenses.gpl2Plus;
    platforms = platforms.linux;
    mainProgram = "alien";
  };
}
