#!/usr/bin/env -S quiz ask

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

- In Bash, every stage of a pipeline runs in its own subshell.
  Modifications to variables inside the `while` loop affect the
  subshell, not the parent shell environment.

- It doesn't handler errors.

- It will work only in BASH 4.0 and newer.
