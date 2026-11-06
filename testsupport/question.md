Consider the following BASH code:

``` bash
sum=0
cat numbers.txt | while read -r num; do
    sum=$((sum + num))
done
echo "Total: $sum"
```

What's wrong with it?

---

- [x] The loop runs in a subshell, so `sum` stays 0 in the parent shell.

- [ ] Inside `$((...))` the variables need a `$`, so `sum + num` is always
  0 + 0.

- [ ] It works only in Bash 4.0 and newer, because older versions lack the
  `$((...))` arithmetic expansion. {-1}
