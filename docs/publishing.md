# Publishing with GitHub CLI

If you are cloning this workspace for the first time and want to publish it
under your own GitHub account, the GitHub CLI can do everything in one step:

```
cd Oni2Rebuilt
gh repo create Oni2Rebuilt --private --source=. --remote=origin --push
```

That initializes the git repository, creates `github.com/<you>/Oni2Rebuilt`,
adds the `origin` remote, and pushes the full workspace. Swap `--private` for
`--public` if you want the repository visible to everyone immediately.
