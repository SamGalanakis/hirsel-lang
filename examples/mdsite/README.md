Render Markdown from `data/content/` recursively into `data/site/`, preserving paths with `.html` appended and generating an escaped index. Raw HTML and unsafe link destinations are omitted by the default renderer.
`hirsel test examples/mdsite/hirsel.json`
`hirsel run examples/mdsite/hirsel.json --grants examples/mdsite/grants.json --record /tmp/mdsite.tape`
`hirsel run --native examples/mdsite/hirsel.json --grants examples/mdsite/grants.json`
`hirsel run examples/mdsite/hirsel.json --replay /tmp/mdsite.tape` (also accepts `--native`).
