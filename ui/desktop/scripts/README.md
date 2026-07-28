# Ponduiny

Put `ponduiny` in your $PATH if you want to launch via:

```
ponduiny .
```

This will open ponduin GUI from any path you specify

# Unregister Deeplink Protocols (macos only)

`unregister-deeplink-protocols.js` is a script to unregister the deeplink protocol used by ponduin like `ponduin://`.
This is handy when you want to test deeplinks with the development version of Ponduin.

# Usage

To unregister the deeplink protocols, run the following command in your terminal:
Then launch Ponduin again and your deeplinks should work from the latest launched ponduin application as it is registered on startup.

```bash
node scripts/unregister-deeplink-protocols.js
```

