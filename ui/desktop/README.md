# ponduin Desktop App

Native desktop app for ponduin built with [Electron](https://www.electronjs.org/) and [ReactJS](https://react.dev/).

# Building and running
ponduin uses [Hermit](https://github.com/cashapp/hermit) to manage dependencies, so you will need to have it installed and activated.

```
git clone git@github.com:PondSec/ponduin.git
cd ponduin
source ./bin/activate-hermit
cd ui/desktop
pnpm install
pnpm run start
```

## Platform-specific build requirements

### Linux
For building on Linux distributions, you'll need additional system dependencies:

**Debian/Ubuntu:**
```bash
sudo apt install dpkg fakeroot
```

**Arch/Manjaro:**
```bash
sudo pacman -S dpkg fakeroot
```

**Fedora/RHEL:**
```bash
sudo dnf install dpkg-dev fakeroot
```

# Building notes

This is an Electron Forge app using Vite and React. The desktop app launches the bundled `ponduin` CLI binary and talks to its ACP server.

## Building for different platforms

### macOS
`pnpm run bundle:default` will give you a ponduin.app/zip which is signed/notarized but only if you set up the env vars as per `forge.config.ts` (you can empty out the section on osxSign if you don't want to sign it) - this will have all defaults.

`pnpm run bundle:preconfigured` will make a ponduin.app/zip signed and notarized, but use the following:

```python
            f"        process.env.PONDUIN_PROVIDER__TYPE = '{os.getenv("PONDUIN_BUNDLE_TYPE")}';",
            f"        process.env.PONDUIN_PROVIDER__HOST = '{os.getenv("PONDUIN_BUNDLE_HOST")}';",
            f"        process.env.PONDUIN_PROVIDER__MODEL = '{os.getenv("PONDUIN_BUNDLE_MODEL")}';"
```

This allows you to set for example PONDUIN_PROVIDER__TYPE to be "databricks" by default if you want (so when people start ponduin.app - they will get that out of the box). There is no way to set an api key in that bundling as that would be a terrible idea, so only use providers that can do oauth (like databricks can), otherwise stick to default ponduin.

### Linux
For Linux builds, first ensure you have the required system dependencies installed (see above), then:

1. Build the Rust binary:
```bash
cd ../..  # Go to project root
cargo build --release -p ponduin-cli --bin ponduin
```

2. Copy the binary to the expected location:
```bash
mkdir -p src/bin
cp ../../target/release/ponduin src/bin/
```

3. Build the application:
```bash
# For ZIP distribution (works on all Linux distributions)
pnpm run make --targets=@electron-forge/maker-zip

# For DEB package (Debian/Ubuntu)
pnpm run make --targets=@electron-forge/maker-deb

# For Flatpak (requires flatpak and flatpak-builder)
pnpm run make --targets=@electron-forge/maker-flatpak
```

The built application will be available in:
- ZIP: `out/make/zip/linux/x64/ponduin-linux-x64-{version}.zip`
- DEB: `out/make/deb/x64/ponduin_{version}_amd64.deb`
- Flatpak: `out/make/flatpak/x86_64/*.flatpak`
- Executable: `out/ponduin-linux-x64/ponduin`

### Windows
Use the existing Windows build process as documented.


# Running with an external ACP backend

From the project root, start the ACP backend:

```bash
PONDUIN_SERVER__SECRET_KEY=test cargo run -p ponduin-cli --bin ponduin -- serve --platform desktop --host 127.0.0.1 --port 3000
```

Then start the desktop app from `ui/desktop`:

```bash
PONDUIN_EXTERNAL_BACKEND=true PONDUIN_EXTERNAL_BACKEND_URL=http://127.0.0.1:3000 PONDUIN_SERVER__SECRET_KEY=test pnpm run start-gui
```
