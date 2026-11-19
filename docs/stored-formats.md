# Stored formats

Every file LANlord stores says which format it is in, and which LANlord wrote it. A file with no
version is version 1, the format from before versions existed. Files that are generated again
each time (rendered units, playbook inputs, caches) carry no version.

| file | where | holds | version kept in |
| --- | --- | --- | --- |
| `settings.yml` | `/opt/lanlord/config` on the gateway | the plain settings, including the network list and the role assignments (`access`) | the top-level keys `lanlord_format` and `lanlord_version` |
| `settings.age` | `/opt/lanlord/config` on the gateway | the secrets, sealed to the gateway's own key | the same keys, inside the sealed yaml |
| `hosts` | the client's config directory | the recent servers | a first line `# lanlord format N, written by LANlord X.Y` |
| `last` | the client's config directory | the connection used last | the same first line |
| `prefs` | the client's config directory | the client preferences | the same first line |
| launcher manifest | the client's data directory | the files `install-launcher` wrote | the same first line |

The playbooks never see the two version keys. Older LANlords read the first line of a text file
as a comment.

## How a file is read

A list of steps, each turning format N into N+1, runs when a file is loaded, so an old file is
always readable in memory. The lists are in `crates/model/src/stored.rs` (`PLAIN_STEPS`,
`SEALED_STEPS`) and next to each client file (`STEPS`). The current format is the number of steps
plus one.

A file with a higher format than this LANlord knows is refused with `written by LANlord X.Y
(format N); upgrade LANlord to use it`, on the client and on the gateway, and is never written
over. There are no down-migrations.

## How a file is stored

`lanlord upgrade` migrates every stored file once, in the operations service on the gateway (the
one that holds the key: it decrypts `settings.age`, migrates, and seals it again). Before it
writes, it copies each file it changes beside itself as `<file>.v<N>.<timestamp>.bak`, where N is
the old version and the timestamp is the gateway's clock (`20260101T120000Z`). If the migration
fails, the copies are put back and the upgrade is rolled back.

Client files are migrated when they are next written.

## Going back to an older LANlord

Put the copies from the upgrade back, then install the older LANlord:

```sh
sudo cp -p /opt/lanlord/config/settings.yml.v1.<timestamp>.bak /opt/lanlord/config/settings.yml
sudo cp -p /opt/lanlord/config/settings.age.v1.<timestamp>.bak /opt/lanlord/config/settings.age
```

Only the files the upgrade changed have a copy. Changes made to the settings after the upgrade
are lost with this.

## How to change a stored format

1. Add a step at the end of the list for that file: a function that turns the old shape into the
   new one, and an entry in the list with a short name. Never reorder or remove a step: a file in
   the wild may be at any version.
2. Add a fixture test: an old file (put it under `crates/model/tests/fixtures/stored/`) in, the
   new shape out.
3. Keep the model crate safe for the browser build (plain data and text, no files or clocks).
   A step that needs to know something about the machine reads it from the `Context`, which the
   upgrade fills in on the gateway.
4. Note the change under `## [Unreleased]` in the changelog.

Every format change adds a step and a fixture test.
