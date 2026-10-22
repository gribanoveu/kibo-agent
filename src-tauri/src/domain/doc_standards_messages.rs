//! Localized "what to do" hints for the standards checker, one function per
//! rule id: messages are plain data, the
//! check logic that decides pass/fail lives in `doc_standards_rules.rs`.
//!
//! Fail texts follow the corporate standard (apiDocumentation v34): a
//! "what's wrong" line (official error name from the weight table) and a
//! "how it should be" line (criteria from §7).

use super::doc_standards::ErrorLanguage;

fn fail(lang: ErrorLanguage, wrong: &str, expected: &str) -> String {
    match lang {
        ErrorLanguage::Ru => format!("Что не так: {wrong}\nКак должно быть: {expected}"),
        ErrorLanguage::En => format!("What's wrong: {wrong}\nHow it should be: {expected}"),
    }
}

const K11_EXPECTED_RU: &str = "В папке methodName — минимальный набор (регистр не важен): diagram.puml / {methodName}.puml / diagrams.puml; {methodName}.adoc; request.adoc / request{methodName}.adoc / {methodName}Request.adoc; response.adoc / response{methodName}.adoc / {methodName}Response.adoc. Дополнительные файлы допустимы.";
const K11_EXPECTED_EN: &str = "A methodName folder must contain this minimum set (case-insensitive): diagram.puml / {methodName}.puml / diagrams.puml; {methodName}.adoc; request.adoc / request{methodName}.adoc / {methodName}Request.adoc; response.adoc / response{methodName}.adoc / {methodName}Response.adoc. Extra files are allowed.";

/// Shared fallback for any rule that reads the main `methodName.adoc` doc
/// when К.1.1 has already flagged it as missing/unmatched — points back at
/// the root cause instead of restating the rule's own unrelated symptom
/// (e.g. "no :toc:" when the real issue is that no file matched at all).
pub fn main_doc_missing(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Основной файл документа не найден (К.1.1: имя файла должно точно совпадать с именем папки метода).",
            K11_EXPECTED_RU,
        ),
        ErrorLanguage::En => fail(
            lang,
            "Main document file not found (K.1.1: the file name must exactly match the method folder name).",
            K11_EXPECTED_EN,
        ),
    }
}

pub fn k1_1(lang: ErrorLanguage, missing_roles: &[&str]) -> String {
    let joined = missing_roles.join(", ");
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            &format!("Отсутствует папка с документацией или название некорректно. Не найдены обязательные файлы: {joined}."),
            K11_EXPECTED_RU,
        ),
        ErrorLanguage::En => fail(
            lang,
            &format!("Documentation folder is missing or named incorrectly. Required files not found: {joined}."),
            K11_EXPECTED_EN,
        ),
    }
}

pub fn k1_2(lang: ErrorLanguage, ambiguous_roles: &[&str]) -> String {
    let joined = ambiguous_roles.join(", ");
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            &format!("Структура папок с документацией некорректна: более одного файла подходит под роль {joined}."),
            "Оставьте ровно один файл на каждую роль (diagram / methodName.adoc / request / response). Дополнительные файлы, не занятые этими ролями, допустимы.",
        ),
        ErrorLanguage::En => fail(
            lang,
            &format!("Folder structure is incorrect: more than one file matches the {joined} role."),
            "Keep exactly one file per role (diagram / methodName.adoc / request / response). Extra files that do not fill those roles are allowed.",
        ),
    }
}

pub fn k1_3(lang: ErrorLanguage, broken: &[String]) -> String {
    const SHOWN: usize = 10;
    let mut list = broken.iter().take(SHOWN).cloned().collect::<Vec<_>>().join("; ");
    if broken.len() > SHOWN {
        list.push_str(&match lang {
            ErrorLanguage::Ru => format!("; и ещё {}", broken.len() - SHOWN),
            ErrorLanguage::En => format!("; and {} more", broken.len() - SHOWN),
        });
    }
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            &format!("Присутствуют некорректные ссылки: {list}."),
            "Каждая ссылка include::, image::, xref: и link: должна вести на существующий файл — путь считается от файла, где стоит ссылка (для image — также от :imagesdir:). Внешние ссылки (http/https) не проверяются: проверка не обращается к сети.",
        ),
        ErrorLanguage::En => fail(
            lang,
            &format!("Broken links: {list}."),
            "Every include::, image::, xref: and link: must point at an existing file, relative to the file holding the link (for image, also to :imagesdir:). External links (http/https) are not checked: the checker does no network access.",
        ),
    }
}

pub fn k2_1(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Оглавление отсутствует.",
            "В {methodName}.adoc должен быть макрос оглавления: `:toc:` или `toc::[]`.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "Table of contents is missing.",
            "The {methodName}.adoc file must contain a TOC macro: `:toc:` or `toc::[]`.",
        ),
    }
}

pub fn k2_2(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Отсутствует раздел «Назначение» или «Описание». Возможно, раздел имеет другое название или не заполнен.",
            "В {methodName}.adoc нужно текстовое описание метода не короче 50 символов (раздел может называться «Назначение», «Описание» или иначе).",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The \"Purpose\" or \"Description\" section is missing. It may have a different title or be empty.",
            "{methodName}.adoc must contain a text description of the method at least 50 characters long (the section may be titled Purpose, Description, or similar).",
        ),
    }
}

pub fn k3_1(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Ссылка на диаграмму отсутствует либо некорректна.",
            "В документе должна быть ссылка на diagram-файл из той же папки (diagram.puml, {methodName}.puml или diagrams.puml), и сам файл не должен быть пустым.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The diagram link is missing or incorrect.",
            "The document must link to a diagram file in the same folder (diagram.puml, {methodName}.puml, or diagrams.puml), and that file must not be empty.",
        ),
    }
}

pub fn k4_1(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Таблица с входными параметрами отсутствует.",
            "Нужна таблица не менее чем из 4 столбцов в {methodName}.adoc или request.adoc. Если входные параметры к методу не применимы, в раздел вставьте: Exceptions - Описание входных параметров исключено из проверки документации.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The input-parameters table is missing.",
            "Add a table with at least 4 columns in {methodName}.adoc or request.adoc. If input parameters do not apply, put this phrase in the section: Exceptions - Описание входных параметров исключено из проверки документации.",
        ),
    }
}

pub fn k4_2(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Таблица с входными параметрами не заполнена.",
            "Каждая ячейка таблицы входных параметров должна быть заполнена. Если в ячейке действительно нечего писать (нет вариантов значений, поле не применимо и т.п.) — поставьте прочерк `-`, не оставляйте её пустой и не выдумывайте содержимое.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The input-parameters table is incomplete.",
            "Every cell in the input-parameters table must be filled in. If a cell genuinely has nothing to put there (no value variants, field not applicable, etc.), put a dash `-` — do not leave it empty and do not invent content.",
        ),
    }
}

pub fn k4_3(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Файл с входными параметрами отсутствует (нет ссылки на request.adoc из папки метода).",
            "В разделе должна быть ссылка на request.adoc из проверяемой папки, например:\n=== Пример запроса\ninclude::./request.adoc[]",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The input-example file is missing (no link to request.adoc in the method folder).",
            "The section must link to request.adoc from the folder under check, for example:\n=== Пример запроса\ninclude::./request.adoc[]",
        ),
    }
}

pub fn k4_4(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Файл с входными параметрами не заполнен.",
            "request.adoc не должен быть пустым: укажите endpoint в формате /path или ${HOST}/path и хотя бы один пример вызова.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The input-example file is incomplete.",
            "request.adoc must not be empty: include an endpoint (/path or ${HOST}/path) and at least one call example.",
        ),
    }
}

/// К.4.4 when `request.adoc` has content but no token the rule accepts as
/// an endpoint — distinct from `k4_4`, so an author with a full URL is not
/// told the file is empty.
pub fn k4_4_no_endpoint(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Файл с входными параметрами не заполнен: endpoint в допустимом формате не найден.",
            "Допустимые форматы endpoint (§7.4.1): /corp-settings-requests-api/tapi/userrequests или ${HOST}/corp-settings-requests-api/tapi/userrequests. Полный URL (https://host/path) и {host}/path без $ стандарт не допускает.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The input-example file is incomplete: no endpoint in an accepted format.",
            "Accepted endpoint formats (§7.4.1): /corp-settings-requests-api/tapi/userrequests or ${HOST}/corp-settings-requests-api/tapi/userrequests. The standard does not accept a full URL (https://host/path) or {host}/path without $.",
        ),
    }
}

pub fn k5_1(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Таблица с выходными параметрами отсутствует.",
            "Если таблица есть, в ней не менее 4 столбцов — в {methodName}.adoc или response.adoc. Если выходные параметры к методу не применимы, в раздел вставьте: Exceptions - Описание выходных параметров исключено из проверки документации.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The output-parameters table is missing.",
            "If a table is present, it must have at least 4 columns — in {methodName}.adoc or response.adoc. If output parameters do not apply, put this phrase in the section: Exceptions - Описание выходных параметров исключено из проверки документации.",
        ),
    }
}

pub fn k5_2(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Таблица с выходными параметрами не заполнена.",
            "Каждая ячейка таблицы выходных параметров должна быть заполнена. Если в ячейке действительно нечего писать (нет вариантов значений, поле не применимо и т.п.) — поставьте прочерк `-`, не оставляйте её пустой и не выдумывайте содержимое.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The output-parameters table is incomplete.",
            "Every cell in the output-parameters table must be filled in. If a cell genuinely has nothing to put there (no value variants, field not applicable, etc.), put a dash `-` — do not leave it empty and do not invent content.",
        ),
    }
}

pub fn k5_3(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Файл с выходными параметрами отсутствует (нет ссылки на response.adoc из папки метода).",
            "В разделе должна быть ссылка на response.adoc из проверяемой папки, например:\n=== Пример ответа\ninclude::./response.adoc[]",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The output-example file is missing (no link to response.adoc in the method folder).",
            "The section must link to response.adoc from the folder under check, for example:\n=== Пример ответа\ninclude::./response.adoc[]",
        ),
    }
}

pub fn k5_4(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Файл с выходными параметрами не заполнен.",
            "response.adoc не должен быть пустым — заполните пример ответа.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The output-example file is incomplete.",
            "response.adoc must not be empty — fill in a response example.",
        ),
    }
}

pub fn k6_1(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Раздел «Алгоритм работы» отсутствует или оформлен сплошным текстом без пунктов.",
            "В {methodName}.adoc раздел «Алгоритм работы» — нумерованный список шагов (первый пункт всегда «Валидация входных параметров»). Каждый пункт списка ниже раскрывается отдельным подразделом с тем же названием и подробным описанием. Не оставляйте алгоритм одним абзацем.",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The \"Algorithm\" section is missing or is a single block of prose without steps.",
            "{methodName}.adoc's \"Алгоритм работы\" section must be a numbered list of steps (the first item is always \"Валидация входных параметров\"). Each list item is then expanded below as its own subsection with the same title and a detailed description. Do not leave the algorithm as one paragraph.",
        ),
    }
}

pub fn k7_1(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => fail(
            lang,
            "Раздел «Обработка ошибок» отсутствует.",
            "В {methodName}.adoc опишите возможные ошибки и коды ответа в разделе «Обработка ошибок».",
        ),
        ErrorLanguage::En => fail(
            lang,
            "The \"Error handling\" section is missing.",
            "{methodName}.adoc must contain a non-empty \"Обработка ошибок\" (error handling) section describing possible errors and response codes.",
        ),
    }
}

pub fn passed(lang: ErrorLanguage) -> String {
    match lang {
        ErrorLanguage::Ru => "Требование выполнено.".to_string(),
        ErrorLanguage::En => "Requirement satisfied.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail_messages(lang: ErrorLanguage) -> Vec<String> {
        vec![
            main_doc_missing(lang),
            k1_1(lang, &["diagram.puml"]),
            k1_2(lang, &["request"]),
            k2_1(lang),
            k2_2(lang),
            k3_1(lang),
            k4_1(lang),
            k4_2(lang),
            k4_3(lang),
            k4_4(lang),
            k5_1(lang),
            k5_2(lang),
            k5_3(lang),
            k5_4(lang),
            k6_1(lang),
            k7_1(lang),
        ]
    }

    #[test]
    fn all_messages_are_non_empty() {
        for lang in [ErrorLanguage::Ru, ErrorLanguage::En] {
            for msg in fail_messages(lang) {
                assert!(!msg.is_empty());
            }
            assert!(!passed(lang).is_empty());
        }
    }

    #[test]
    fn fail_messages_have_wrong_and_expected_markers() {
        for msg in fail_messages(ErrorLanguage::Ru) {
            assert!(msg.contains("Что не так:"), "{msg}");
            assert!(msg.contains("Как должно быть:"), "{msg}");
        }
        for msg in fail_messages(ErrorLanguage::En) {
            assert!(msg.contains("What's wrong:"), "{msg}");
            assert!(msg.contains("How it should be:"), "{msg}");
        }
    }

    #[test]
    fn empty_table_cell_messages_tell_to_use_a_dash() {
        for msg in [k4_2(ErrorLanguage::Ru), k5_2(ErrorLanguage::Ru)] {
            assert!(msg.contains("прочерк `-`"), "{msg}");
        }
        for msg in [k4_2(ErrorLanguage::En), k5_2(ErrorLanguage::En)] {
            assert!(msg.contains("dash `-`"), "{msg}");
        }
    }
}
