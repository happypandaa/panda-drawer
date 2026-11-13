---
title: Work with your AI
description: Open an app's project folder in your own AI tool, send the prompt, and tell it what to change.
lang: en
copy_code: true
---

# Work with your AI

Start with an AI tool you already have installed that can read and edit files on your computer.

## Make your first change

1. **Open the app details in Panda Drawer.** Under **Make it your own**, click a button such as **Change with WorkBuddy**. On macOS you can also click **Change it your way** in the app window's title bar.

{% screenshot s05-clack-modify.webp 1040 454 Clack app details on macOS in Chinese, showing Change with WorkBuddy, Only copy the prompt, and Change AI tool %}

2. **Send the prepared prompt in your AI tool.** Check that it has opened the right project folder. If the prompt is already filled in, review it and send it. If the tool did not open or the prompt is missing, return to the app details and click **Only copy the prompt**. Open the app's project folder in your AI tool, then paste and send the prompt.
3. **Tell the AI what you want to change.** Start with a small change you can try yourself. For example, ask it to make the practice text larger in Clack.

```text
Make the passage text I type along with in Clack a little larger.
Keep regular practice and my existing history. Tell me where I can see the change.
```

4. **Return to the app and try it.** Save any unfinished work first. On macOS, click **Reload** in the title bar. If your AI asks you to run **Prepare app** first, close the app window, finish preparation, then open it again. [What to do if the change doesn't appear](/en/wiki/faq/)

{% screenshot s06-reload.webp 474 66 The Reload and Change it your way buttons in the macOS app title bar, shown in Chinese %}

For a new app, the button says **Start with** followed by your tool's name, such as **Start with WorkBuddy**. When your AI asks what you want to make, agree on what the first version will do before it starts. [Make a notes app](/en/tutorials/create-your-app/)

## Choose a tool

On macOS, Panda Drawer detects installed tools and asks which one to use the first time you hand off an app. Click **Check again** after installing a new tool. To switch tools later, use **My AI tool** in Settings or **Change AI tool** in the app details.

| Method | Tools | What to do next |
| --- | --- | --- |
| Open with a prepared prompt | Codex, Claude Code, Claude desktop, WorkBuddy | Review the project folder and prepared prompt, then send it. |
| Open and paste the prompt | Cursor, VS Code, Trae, Trae CN, Windsurf, Kiro, CodeBuddy, CodeBuddy CN, Tongyi Lingma, Qoder | Paste the prompt into a conversation that can edit files. With Qoder, choose or confirm the project folder. |
| Open manually | **Other / I am not sure yet** | Copy the prompt, open the folder in your AI tool, then paste and send it. |

For one-click opening to work with Claude Code, first run `claude` in your terminal and send a message. Install your tool before handing off an app. You can use **Only copy the prompt** whenever Panda Drawer cannot open the tool or fill in the prompt.

Browser-based chats such as Doubao and ChatGPT on the web cannot directly edit files on your computer. Choose a tool that can access your local project folder.

## Use your AI for everyday work

Gleaner shows you the prompt to send to your AI. Click **交给我的 AI** (hand off to my AI), or copy the prompt into a tool that can search the web and read and write files in Gleaner's project folder. Ask your AI to gather new information, then return to Gleaner to read it.

To change Gleaner's layout or features, click **Change it your way** in its title bar on macOS, or use the **Change with** button in the app details. [How to use Gleaner](/en/apps/gleaner/)

## Work with Trae, Qoder, or similar tools

1. Click **Change with Trae**, **Change with Trae CN**, or **Change with Qoder** in the app details. Panda Drawer copies the prompt. Trae tries to open the project folder. In Qoder, choose or confirm the folder yourself.
2. Open an AI conversation that can edit files, paste the full prompt, and confirm the project folder before sending it.
3. After the AI introduces the app, explain what you want changed. If it proposes a plan, check what it will change and whether it affects existing data before you agree.

If Panda Drawer cannot open the tool, click **Only copy the prompt**, then open the project folder and paste the prompt yourself.

<span id="choose-the-project-folder"></span>

## Choose the project folder

Open the entire folder that contains `panda.json`. You can find its location in the app details and in the prepared prompt.

```text
Your project folder/
├── panda.json   Basic app information
├── README.md    Instructions
├── …            App files
└── data/        File data, if present
```

If the project folder moves, get a fresh prompt from the app details and open the new location.

## What the prompt contains

The prompt includes the app name, the project folder location, and the location of the platform instructions. Send it as prepared so your AI knows which files to read.

## Work on Windows

Click **Only copy the prompt** in the app details. Open the project folder in your AI tool and send the prompt manually. [Windows steps](/en/wiki/getting-started/#windows)

[Customize Clack](/en/tutorials/remix-an-app/) · [Make your own app](/en/tutorials/create-your-app/)
