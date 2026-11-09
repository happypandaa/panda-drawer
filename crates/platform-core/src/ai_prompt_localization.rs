//! Localize only complete, known platform handoffs; inserted project values stay opaque.
use super::{
    render_with_rules, BACKEND_CONTEXT, BACKEND_CONTEXT_EN, ENGLISH_RULES, RULES,
    TRANSLATED_TEMPLATES,
};
use regex::Regex;
use std::{collections::BTreeMap, sync::LazyLock};

struct Scaffold {
    pattern: Regex,
    names: Vec<String>,
    english: &'static str,
}

static SCAFFOLDS: LazyLock<Vec<Scaffold>> = LazyLock::new(|| {
    let parameter = Regex::new(r"\{([A-Za-z][A-Za-z0-9_]*)\}").unwrap();
    TRANSLATED_TEMPLATES
        .iter()
        .copied()
        .chain(std::iter::once((BACKEND_CONTEXT, BACKEND_CONTEXT_EN)))
        .map(|(source, english)| {
            let mut pattern = String::from(r"\A");
            let mut offset = 0;
            let mut names = Vec::new();
            for capture in parameter.captures_iter(source) {
                let whole = capture.get(0).unwrap();
                pattern.push_str(&regex::escape(&source[offset..whole.start()]));
                let name = &capture[1];
                if let Some((_, rule)) = RULES.iter().find(|(key, _)| *key == name) {
                    pattern.push_str(&regex::escape(rule));
                } else {
                    names.push(name.to_owned());
                    pattern.push_str(match name {
                        "nameLine" => r"((?:应用：[^\n]*\n)?)",
                        "descriptionLine" => r"((?:简介：[^\n]*\n)?)",
                        _ => r"((?s:.*?))",
                    });
                }
                offset = whole.end();
            }
            pattern.push_str(&regex::escape(&source[offset..]));
            pattern.push_str(r"\z");
            Scaffold {
                pattern: Regex::new(&pattern).unwrap(),
                names,
                english,
            }
        })
        .collect()
});

/// Call only for platform-generated text. App-supplied text bypasses localization entirely.
pub fn localize_handoff(text: &str, locale: &str) -> String {
    if locale != "en" {
        return text.to_owned();
    }
    for scaffold in SCAFFOLDS.iter() {
        let Some(captures) = scaffold.pattern.captures(text) else {
            continue;
        };
        let mut values = BTreeMap::new();
        let mut consistent = true;
        for (index, name) in scaffold.names.iter().enumerate() {
            let value = &captures[index + 1];
            if values
                .insert(name.as_str(), value)
                .is_some_and(|previous| previous != value)
            {
                consistent = false;
                break;
            }
        }
        if !consistent {
            continue;
        }
        let localized: Vec<_> = values
            .into_iter()
            .map(|(name, value)| {
                let value = match name {
                    "nameLine" if !value.is_empty() => {
                        format!("Application: {}\n", &value["应用：".len()..value.len() - 1])
                    }
                    "descriptionLine" if !value.is_empty() => {
                        format!("Description: {}\n", &value["简介：".len()..value.len() - 1])
                    }
                    "skillPath" | "cliPath" if value == "当前平台缺少安装资源，暂时无法提供" => {
                        "Installation resources are missing from the current platform, so instructions are temporarily unavailable".to_owned()
                    }
                    "backendContext" if !value.is_empty() => localize_handoff(value, locale),
                    _ => value.to_owned(),
                };
                (name, value)
            })
            .collect();
        let values: Vec<_> = localized
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        return render_with_rules(scaffold.english, &values, &ENGLISH_RULES);
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_prompt_rules::render;

    #[test]
    fn unified_handoffs_translate_the_scaffold_and_keep_inserted_values_verbatim() {
        let values = [
            ("nameLine", "应用：中文应用 {projectRoot}\n"),
            ("descriptionLine", "简介：中文简介 {instructions}\n"),
            ("projectRoot", "/disposable/中文目录/{skillPath}"),
            ("skillPath", "/disposable/平台说明/{instructions}/SKILL.md"),
            ("cliPath", "/disposable/中文命令行/{nameLine}"),
            ("logsPath", "/disposable/中文日志/{projectRoot}"),
            ("instructions", "中文规则/{cliPath}.md"),
            ("backendContext", ""),
        ];
        for (source, english) in TRANSLATED_TEMPLATES {
            let text = render(source, &values);
            let translated = localize_handoff(&text, "en");
            let mut expected_values = values;
            expected_values[0].1 = "Application: 中文应用 {projectRoot}\n";
            expected_values[1].1 = "Description: 中文简介 {instructions}\n";
            assert_eq!(
                translated,
                render_with_rules(english, &expected_values, &ENGLISH_RULES)
            );
            assert!(translated.contains(values[2].1));
            assert!(translated.contains(values[3].1));
            assert!(translated.contains(values[4].1));
            assert_eq!(localize_handoff(&text, "zh-CN"), text);
            assert_eq!(localize_handoff(&translated, "en"), translated);
        }
    }

    #[test]
    fn backend_context_translates_labels_and_preserves_paths_and_id() {
        let context = render(
            BACKEND_CONTEXT,
            &[
                ("logsPath", "/disposable/中文日志/{appId}"),
                ("cliPath", "/disposable/cli"),
                ("appId", "fixture-app"),
            ],
        );
        let text = render(
            TRANSLATED_TEMPLATES[0].0,
            &[
                ("nameLine", ""),
                ("projectRoot", "/disposable"),
                ("skillPath", "/skill"),
                ("cliPath", "/cli"),
                ("backendContext", &context),
            ],
        );
        let translated = localize_handoff(&text, "en");
        assert!(translated.contains("Backend logs: /disposable/中文日志/{appId}"));
        assert!(
            translated.contains("Inspect logs: /disposable/cli app logs --app fixture-app --json")
        );
        assert!(!translated.contains("后台日志："));
    }

    #[test]
    fn partial_or_modified_scaffolds_are_preserved() {
        let text = render(TRANSLATED_TEMPLATES[0].0, &[("projectRoot", "/disposable")]);
        for changed in [
            format!("{text}extra"),
            text.replace(RULES[0].1, "a different rule"),
            "应用自己提供的文字 {projectRoot}".into(),
        ] {
            assert_eq!(localize_handoff(&changed, "en"), changed);
        }
    }
}
