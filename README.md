<div align="center">

<img src="icons/copi-logo.svg" alt="Copi" width="96" height="96">

# Copi

**Your local clipboard copilot.**

Search by meaning, all locally.

<br>

[![Release](https://img.shields.io/github/v/release/Pixlox/copi?style=for-the-badge&label=latest&color=0A84FF&labelColor=1a1a1f)](https://github.com/Pixlox/copi/releases)
[![Build](https://img.shields.io/github/actions/workflow/status/Pixlox/copi/release.yml?style=for-the-badge&label=build&labelColor=1a1a1f)](https://github.com/Pixlox/copi/actions/workflows/release.yml)
[![Downloads](https://img.shields.io/github/downloads/Pixlox/copi/total?style=for-the-badge&label=downloads&color=34C759&labelColor=1a1a1f)](https://github.com/Pixlox/copi/releases)
[![Last Commit](https://img.shields.io/github/last-commit/Pixlox/copi?style=for-the-badge&label=updated&color=AF52DE&labelColor=1a1a1f)](https://github.com/Pixlox/copi/commits/main)
[![Stars](https://img.shields.io/github/stars/Pixlox/copi?style=for-the-badge&color=FFD60A&labelColor=1a1a1f)](https://github.com/Pixlox/copi/stargazers)
[![Issues](https://img.shields.io/github/issues/Pixlox/copi?style=for-the-badge&color=FF9500&labelColor=1a1a1f)](https://github.com/Pixlox/copi/issues)
[![License](https://img.shields.io/github/license/Pixlox/copi?style=for-the-badge&color=5AC8FA&labelColor=1a1a1f)](LICENSE)
<br>

<img src="https://github.com/user-attachments/assets/2b2f5d49-d60b-48de-9ded-6dd75c0c670e" alt="Copi demo" width="680">

<br>
<br>


[**Download for macOS**](https://github.com/Pixlox/copi/releases/latest) · [**Download for Windows**](https://github.com/Pixlox/copi/releases/latest) · [**Report an issue**](https://github.com/Pixlox/copi/issues/new)

</div>

---

## Why Copi?

Copi's a simple clipboard manager, with one distinct feature: semantic search through an embeddings model.

Type *"japanese from arc about 3 weeks ago"* and it finds the japanese you copied three weeks ago. Type *"auth code from slack"* and it surfaces the 2FA code from this morning. Type *"that youtube video"* and it knows you mean a URL. 

Copi _understands_ what you mean, and it does this entirely offline using a small AI model that runs on your machine. All free.

---

## Features

<table>
<tr>
<td width="50%">

**🧠 Semantic search**

Understands your intent, not just keywords. "youtube video from last night", finds that exact video you copied.

</td>
<td width="50%">

**🖼 OCR on images**

Screenshots become searchable. Copi reads the text and indexes it.

</td>
</tr>
<tr>
<td width="50%">

**⏱ Natural time expressions**

"yesterday afternoon", "last Friday", "10 minutes ago", "from this morning", all understood.

</td>
<td width="50%">

**📁 Collections**

Organise your clips into folders. Copy them, manage them, use them.

</td>
</tr>
<tr>
<td width="50%">

**🔄 Transforms**

Uppercase, title case, JSON pretty print, extract URLs, deduplicate lines, sort, all easy.

</td>
<td width="50%">

**🔒 Privacy first**

Exclude any app by name or bundle ID. Password managers don't capture if you don't want them to. It's all local, too.

</td>
</tr>
</table>

<table> 
<td width="50%">

**🔄 Copi Sync and Wormhole (new!)**

Sync your clipboard, pins, and collections across your devices instantly with Copi. Works cross-platform, fast and secure. Send larger files across your devices through Copi Wormhole. 

</td>

<td width="20%">
<img src="https://github.com/user-attachments/assets/3e281200-a7c1-48de-be80-6230d585c324" alt="Copi Wormhole demo" width="250">

</td>
</table>


---

## Install

### macOS

Download the `.dmg` from [Releases](https://github.com/Pixlox/copi/releases/latest), drag Copi to Applications.

**NOTE:**
macOS users must remove the quarantine flag from Copi before using it, or macOS will say that it is damaged.   

To do this, simply run this in your terminal: 

`xattr -rd com.apple.quarantine /path/to/copi.app`  

Then, right-click -> open it.

### Windows

Download the `.msi` from [Releases](https://github.com/Pixlox/copi/releases/latest) and run the installer.

### First run

<!-- SCREENSHOT: the setup window showing "Downloading AI model..." with the 
     animated progress bar. The cinematic onboarding looks great here. -->

On first launch, Copi needs to download the embedding model.

---

## How it works

- Content classification - text, URL, code or image
- OCR via platform specific APIs (Apple Vision on macOS, Windows Media OCR on Windows)
- [Multilingual-e5-small](https://huggingface.co/intfloat/multilingual-e5-small) embeddings to semantically search your clips. All offline via ONNX runtime.
- SQLite to store everything and vector search. Blends BM25 keyword scores, vector cosine distance, recency, and more.

---

## Building from source

You'll need Rust (stable), Node.js 18+, and the Tauri CLI.

```bash
git clone https://github.com/Pixlox/copi
cd copi
npm install
npm run tauri dev
```

---

## Status

Copi is in alpha. It'll work well day to day but there may be rough edges.

If you find a bug, [open an issue](https://github.com/Pixlox/copi/issues/new).

PRs are welcome.

---

<div align="center">

Made with <3 by [Pixlox](https://pixlox.me) and [Megumi Labs](https://github.com/MegumiLabs) 

</div>
