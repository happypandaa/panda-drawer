"""Interactively save release API credentials outside the source repository."""
import getpass
import json
import os
from pathlib import Path


def main():
    directory = Path.home() / ".config" / "panda-drawer"
    destination = directory / "release-credentials.json"
    if destination.exists() or destination.is_symlink():
        raise SystemExit("配置文件已存在，可直接提供这个路径：" + str(destination))

    github = getpass.getpass("GitHub token（输入不会显示）: ").strip()
    gitee = getpass.getpass("Gitee token（输入不会显示）: ").strip()
    if not github or not gitee:
        raise SystemExit("两枚令牌都需要填写，尚未保存文件。")

    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    if directory.is_symlink():
        raise SystemExit("配置目录不能是符号链接。")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(destination, flags, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump({"github": github, "gitee": gitee}, output)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    print("已保存到 " + str(destination))
    print("可以告诉 Codex 配置已完成，令牌内容无需发送。")


if __name__ == "__main__":
    main()
