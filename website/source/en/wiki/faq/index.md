---
title: Frequently asked questions
description: Find answers about adding, configuring, changing, reloading, and deleting Panda Drawer apps.
lang: en
---

# Frequently asked questions

## Adding and configuring apps

### Can I add any link?

Use the GitHub or Gitee app URL provided by its author. An ordinary webpage or sharing link may not work. For your first app, choose one under **Recommended apps**.

If you see **Hand to AI** after a failure, you can ask your own AI to check what went wrong and explain what to do next.

### Do I need to install other tools or databases?

Panda Drawer prepares the tools apps need to run. Check the [app pages](/en/apps/) for any accounts, API keys, or AI tools a particular app needs.

### Where do I enter API keys? Do they cost anything?

After adding an app that needs setup, you'll see **App configuration**. To update it later, choose **Edit** under **App configuration** in the app details. Enter the key in Panda Drawer and click **Save configuration**. If the app is open, reload it to apply the new settings.

Clack needs no account or key. Readio needs DeepSeek and Fish Audio keys. Radio Bob needs a DeepSeek key and can optionally use Fish Audio for speech. See the [app pages](/en/apps/) for requirements and pricing information.

### Can I use apps offline?

Clack's typing practice works offline after installation. Translation, generated speech, and online music selection need an internet connection. Check the [app pages](/en/apps/) for each app's requirements.

## Changing apps

<span id="ai-says-it-finished-why-hasnt-the-app-changed"></span>

### My AI finished. Why hasn't the app changed?

Save any unfinished work first. On macOS, click **Reload** in the app window's title bar. See the [Windows steps](/en/wiki/getting-started/#windows) if you're using Windows.

{% screenshot s06-reload.webp 474 66 The Reload button in the macOS app title bar, shown in Chinese %}

If your AI asks you to prepare the app first, close its window and click **Prepare app** in the app details. Reopen it once preparation finishes. If the app is already open, clicking **Open** brings its existing window to the front. If you see an error, share the message with the AI that's making the change.

### What if my AI cannot read the files?

In the app details, click **Only copy the prompt** again. Open the entire project folder named in the prompt in your AI tool, then paste and send the prompt. Use a conversation that can read and edit files in that folder. [How to choose the project folder](/en/wiki/ai-workspace/#choose-the-project-folder)

### Can I undo a change?

Panda Drawer reminds your AI to save the current version before editing. To undo an edit, ask your AI to revert its last change and restore the previous version.

That version covers how the app works. Back up data such as practice records separately. Before changing data, your AI should explain the impact and ask for your approval. [Platform rules](/en/wiki/platform-rules/)

## Data and deletion

### Where is my data stored?

App data stays on your computer and can be stored in two places.

- **Browser data** belongs to the app window. Deleting the app clears that data. Adding the same project folder again will not recover it.
- **Files inside the project folder** stay with that folder. What happens when you delete the app depends on how you added it.

Keep a separate copy of important data. Readio, for example, can export a backup from its **⋯** menu and import it again later. [Data, configuration, logs, and deletion](/en/wiki/data-and-config/)

### What happens when I close a window?

Closing an app window stops the app. When you reopen it, it uses the last saved data. Opening an app that is already running brings its window to the front.

On macOS, closing the Panda Drawer management window hides it. Click its Dock icon to return. To quit Panda Drawer and close all its apps, use the app menu or press `Cmd+Q`. On Windows, closing the management window quits Panda Drawer.

### Does deleting an app delete its files?

Check the confirmation screen before deleting. It lists what will be removed.

| Location selected when adding the app | Project folder and its files | App settings, API keys, and browser data |
| --- | --- | --- |
| A folder you chose, such as **Local folder**, an empty folder for a new app, or an empty folder for Gleaner | Kept | Removed |
| Panda Drawer's default location, such as a recommended app or one added from a link | Removed | Removed |

Deletion is permanent. Downloads and backups you saved elsewhere are kept. If you scheduled Gleaner or another app in your AI tool, stop those tasks there first. [Details about data and deletion](/en/wiki/data-and-config/)

### Does uninstalling delete my apps?

Uninstalling Panda Drawer keeps your apps and data. If you want to remove all local app content, go to Settings, choose **Clear all local project content**, review what will be removed, and confirm before quitting and uninstalling.

If you use a third-party cleanup tool, review what it will remove and save any files you need before continuing.

## Downloads and system requirements

### Which systems are supported? Where can I download Panda Drawer?

Check [Download and system requirements](/en/download/) for the current release status and supported systems.

### What if downloads are slow in China?

Under **Download source** in Settings, choose **China (Gitee)**, or leave it on **Automatic (recommended)** to let Panda Drawer choose. You can also switch to **Global (GitHub)**. This setting applies to recommended apps.

[Back to Help](/en/wiki/) · [Browse tutorials](/en/tutorials/)
