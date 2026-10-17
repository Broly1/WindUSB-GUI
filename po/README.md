# Translating WindUSB-GUI 🌍

Translations are standard gettext `.po` files. You can edit them with any text
editor or with a tool like [Poedit](https://poedit.net/), Lokalize or GTranslator.
No Rust knowledge is needed.

| File | Language |
| --- | --- |
| `windusb.pot` | Template (English source strings, do not translate this one) |
| `pt_BR.po` | Brazilian Portuguese |
| `es.po` | Spanish |

## Add a new language

1. Copy the template, naming the copy after the language code:

   ```bash
   cp po/windusb.pot po/fr.po        # French
   cp po/windusb.pot po/pt_PT.po     # European Portuguese
   ```

   Use the plain code (`fr`, `de`, `it`) unless the language needs a region
   (`pt_BR`, `zh_CN`). A plain `es.po` is also used for `es_MX`, `es_AR`, etc.

2. Set `"Language: fr\n"` in the header and fill in every `msgstr ""`.
   Leave an entry empty if you are unsure: it simply shows in English.

3. Build (`./build.sh` or `cargo build --release`). Every `.po` file in this
   folder is picked up automatically, no code changes needed.

4. Check it, then open a pull request with your `.po` file:

   ```bash
   cargo test                                    # validates all .po files
   LANGUAGE=fr ./target/release/windusb-gui      # or the AppImage
   ```

## Rules

* **Keep every `{}`.** It is replaced with a value at runtime (a drive like
  `/dev/sdb`, a number of MB, an error message). Your translation must contain
  the same number of `{}`, in the same order. If it doesn't, that line is
  ignored and shown in English (`cargo test` will tell you).
* **Keep `_` in button labels** like `_Cancel`. The letter after `_` becomes the
  keyboard shortcut (Alt+letter). You can move it to another letter.
* Lines starting with `#.` are hints for translators.
* Entries marked `#, fuzzy` are ignored until you remove that mark.
* Keep file names (`install.wim/esd`), `FAT32`, `/dev`, `7z`, `lsblk`, `sgdisk`
  and `pkexec` as they are.

## Update an existing language

When new strings are added to `windusb.pot`, merge them into your file:

```bash
msgmerge --update po/es.po po/windusb.pot
```

or in Poedit use *Translation → Update from POT file*.

## For developers

* Wrap new GUI text with `tr("...")`, or `trf("... {} ...", &[&value])` when it
  contains values, then add the English string as a `msgid` to `windusb.pot`
  (and to the existing `.po` files if you can translate it).
* The root helper (`--flash`) is **not** translated and its output must stay in
  English. The GUI translates helper messages when displaying them, by
  matching them against the `msgid`s (`{}` matches any value). If you change
  a helper message, update its `msgid` here, otherwise it just falls back to
  English.
* The language is taken from `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` or `LANG`.
