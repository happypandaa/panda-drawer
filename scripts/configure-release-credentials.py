"""Interactively create or update release API credentials outside the repository."""
import getpass
import json
import os
from pathlib import Path
import stat
import tempfile


def read_current(destination):
    try:
        info = destination.lstat()
    except FileNotFoundError:
        return {}
    if not stat.S_ISREG(info.st_mode) or info.st_size > 16 * 1024:
        raise SystemExit("配置文件必须是普通文件，尚未修改。")
    if os.name != "nt" and (info.st_uid != os.getuid() or info.st_mode & 0o077):
        raise SystemExit("配置文件必须仅当前用户可读写，尚未修改。")
    try:
        saved = json.loads(destination.read_text())
        if not isinstance(saved, dict) or not all(
            isinstance(saved.get(key), str) and saved[key].strip()
            for key in ("github", "gitee")
        ):
            raise ValueError()
        return {key: saved[key].strip() for key in ("github", "gitee")}
    except (OSError, ValueError):
        raise SystemExit("配置文件无法读取或格式错误，尚未修改。") from None


def main():
    directory = Path.home() / ".config" / "panda-drawer"
    destination = directory / "release-credentials.json"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    if directory.is_symlink():
        raise SystemExit("配置目录不能是符号链接。")
    current = read_current(destination)
    if current:
        print("已有配置：输入新令牌进行替换，直接回车保留对应的原令牌。")
    values = {}
    try:
        for key, label in (("github", "GitHub"), ("gitee", "Gitee")):
            prompt = "（输入隐藏，留空保留）" if current else "（输入不会显示）"
            values[key] = getpass.getpass(label + " token" + prompt + ": ").strip() or current.get(key, "")
    except (KeyboardInterrupt, EOFError):
        raise SystemExit("已取消，原配置未修改。") from None
    if not all(values.values()):
        raise SystemExit("首次配置需要两枚令牌，尚未保存文件。")
    if values == current:
        print("未更改配置，原令牌继续有效。")
        return

    # Write a private sibling first. Interruptions leave the original intact;
    # replacement never creates a backup containing retired credentials.
    descriptor, temporary = tempfile.mkstemp(prefix=".release-credentials-", dir=directory)
    try:
        with os.fdopen(descriptor, "w") as output:
            json.dump(values, output)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, destination)
    finally:
        Path(temporary).unlink(missing_ok=True)
    print("已保存到 " + str(destination))
    print("可以告诉 Codex 配置或更新已完成，令牌内容无需发送。")


if __name__ == "__main__":
    main()
