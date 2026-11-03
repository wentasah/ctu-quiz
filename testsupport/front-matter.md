+++
close-after = 30
shuffle = true
+++

# Exit status

What does the following shell command print?

``` bash
false | true; echo $?
```

---

- [x] `0`
- [ ] `1`
- [ ] Nothing, `false` stops the pipeline.

The exit status of a pipeline is that of its last command, unless
`set -o pipefail` is in effect; then it is `1` here.
