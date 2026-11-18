# smectl

Command line for [smed](https://github.com/thomschlie/smed), a Linux driver daemon for the
3Dconnexion SpaceMouse Enterprise:

```bash
smectl monitor      # every event as a JSON line
smectl state        # the device and the rotation lock
smectl lock on|off  # the rotation lock
smectl lcd --brightness 60 --theme light   # until smed restarts
smectl demo-page    # a sample page on the LCD while it runs
smectl screenshot lcd.png   # what smed draws on the LCD now, as PNG
```

It needs a running smed (`cargo install smed`, and the setup in smed's
[README](https://github.com/thomschlie/smed#install-from-source)).

Not affiliated with or endorsed by 3Dconnexion.

## Licence

MIT or Apache-2.0, at your option.
