// SPDX-FileCopyrightText: 2022 Klarälvdalens Datakonsult AB, a KDAB Group company <info@kdab.com>
// SPDX-FileContributor: Andrew Hayzen <andrew.hayzen@kdab.com>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    generator::{
        cpp::{fragment::CppFragment, qobject::GeneratedCppQObjectBlocks},
        naming::{qobject::QObjectName, signals::QSignalName},
        utils::cpp::syn_type_to_cpp_type,
    },
    parser::{
        mappings::ParsedCxxMappings, parameter::ParsedFunctionParameter, signals::ParsedSignal,
    },
};
use indoc::formatdoc;
use syn::Result;

/// Combined output of possible parameter lines to be used
struct Parameters {
    types_closure: String,
    types_signal: String,
    values_closure: String,
}

/// Representation of the self pair
///
/// This allows for *this being passed or a parameter in the arguments
struct SelfValue<'a> {
    ident: &'a str,
    ty: &'a str,
}

/// From given parameters, mappings, and self value constructor the combined parameter lines
fn parameter_types_and_values(
    parameters: &[ParsedFunctionParameter],
    cxx_mappings: &ParsedCxxMappings,
    self_value: SelfValue,
) -> Result<Parameters> {
    let mut parameter_types_closure = vec![];
    let mut parameter_values_closure = vec![];

    for parameter in parameters {
        let cxx_ty = syn_type_to_cpp_type(&parameter.ty, cxx_mappings)?;
        let ident_str = parameter.ident.to_string();
        parameter_types_closure.push(format!("{cxx_ty} {ident_str}",));
        parameter_values_closure.push(format!("::std::move({ident_str})"));
    }

    let parameters_types_signal = parameter_types_closure.join(", ");

    // Insert the extra argument into the closure
    parameter_types_closure.insert(0, format!("{ty}&", ty = self_value.ty));
    parameter_values_closure.insert(0, self_value.ident.to_owned());

    Ok(Parameters {
        types_closure: parameter_types_closure.join(", "),
        types_signal: parameters_types_signal,
        values_closure: parameter_values_closure.join(", "),
    })
}

/// Generate C++ blocks for a free signal on an existing QObject (not generated by CXX-Qt), eg QPushButton::clicked
pub fn generate_cpp_free_signal(
    signal: &ParsedSignal,
    cxx_mappings: &ParsedCxxMappings,
) -> Result<CppFragment> {
    // Prepare the idents we need
    let qobject_ident = signal.qobject_ident.to_string();
    let qobject_ident_namespaced = cxx_mappings.cxx(&qobject_ident);
    let idents = QSignalName::from(signal);
    let signal_ident = idents.name.cpp.to_string();
    // TODO: in the future we might improve the naming of the methods
    // to avoid collisions (maybe use a separator similar to how CXX uses $?)
    let connect_ident = idents.connect_name.cpp.to_string();

    // Retrieve the parameters for the signal
    let parameters = parameter_types_and_values(
        &signal.parameters,
        cxx_mappings,
        SelfValue {
            ident: "self",
            ty: &qobject_ident_namespaced,
        },
    )?;
    let parameters_types_closure = parameters.types_closure;
    let parameters_types_signal = parameters.types_signal;
    let parameters_values_closure = parameters.values_closure;

    Ok(CppFragment::Pair {
        header: formatdoc!(
            r#"
            ::QMetaObject::Connection
            {qobject_ident}_{connect_ident}({qobject_ident_namespaced}& self, ::rust::Fn<void({parameters_types_closure})> func, ::Qt::ConnectionType type);
            "#,
        ),
        source: formatdoc! {
            r#"
            ::QMetaObject::Connection
            {qobject_ident}_{connect_ident}({qobject_ident_namespaced}& self, ::rust::Fn<void({parameters_types_closure})> func, ::Qt::ConnectionType type)
            {{
                return ::QObject::connect(
                    &self,
                    &{qobject_ident_namespaced}::{signal_ident},
                    &self,
                    [&, func = ::std::move(func)]({parameters_types_signal}) {{
                        const ::rust::cxxqtlib1::MaybeLockGuard<{qobject_ident_namespaced}> guard(self);
                        func({parameters_values_closure});
                    }},
                    type);
            }}
            "#,
        },
    })
}

pub fn generate_cpp_signals(
    signals: &Vec<ParsedSignal>,
    qobject_idents: &QObjectName,
    cxx_mappings: &ParsedCxxMappings,
) -> Result<GeneratedCppQObjectBlocks> {
    let mut generated = GeneratedCppQObjectBlocks::default();
    let qobject_ident = qobject_idents.cpp_class.cpp.to_string();

    for signal in signals {
        // Prepare the idents
        let idents = QSignalName::from(signal);
        let signal_ident = idents.name.cpp.to_string();
        let connect_ident = idents.connect_name.cpp.to_string();

        // Generate the parameters
        let parameters = parameter_types_and_values(
            &signal.parameters,
            cxx_mappings,
            SelfValue {
                ident: "*this",
                ty: &qobject_ident,
            },
        )?;
        let parameters_types_closure = parameters.types_closure;
        let parameters_types_signal = parameters.types_signal;
        let parameters_values_closure = parameters.values_closure;

        // Generate the Q_SIGNAL if this is not an existing signal
        if !signal.inherit {
            generated.methods.push(CppFragment::Header(format!(
                "Q_SIGNAL void {signal_ident}({parameters_types_signal});"
            )));
        }

        generated.methods.push(CppFragment::Pair {
            header: format!(
                "::QMetaObject::Connection {connect_ident}(::rust::Fn<void({parameters_types_closure})> func, ::Qt::ConnectionType type);",
            ),
            source: formatdoc! {
                r#"
                ::QMetaObject::Connection
                {qobject_ident}::{connect_ident}(::rust::Fn<void({parameters_types_closure})> func, ::Qt::ConnectionType type)
                {{
                    return ::QObject::connect(this,
                        &{qobject_ident}::{signal_ident},
                        this,
                        [&, func = ::std::move(func)]({parameters_types_signal}) {{
                            const ::rust::cxxqtlib1::MaybeLockGuard<{qobject_ident}> guard(*this);
                            func({parameters_values_closure});
                        }},
                        type);
                }}
                "#,
            },
        });
    }

    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::generator::naming::{qobject::tests::create_qobjectname, CombinedIdent};
    use crate::parser::parameter::ParsedFunctionParameter;
    use indoc::indoc;
    use pretty_assertions::assert_str_eq;
    use quote::format_ident;
    use syn::parse_quote;

    #[test]
    fn test_generate_cpp_signals() {
        let signals = vec![ParsedSignal {
            method: parse_quote! {
                fn data_changed(self: Pin<&mut MyObject>, trivial: i32, opaque: UniquePtr<QColor>);
            },
            qobject_ident: format_ident!("MyObject"),
            mutable: true,
            parameters: vec![
                ParsedFunctionParameter {
                    ident: format_ident!("trivial"),
                    ty: parse_quote! { i32 },
                },
                ParsedFunctionParameter {
                    ident: format_ident!("opaque"),
                    ty: parse_quote! { UniquePtr<QColor> },
                },
            ],
            ident: CombinedIdent {
                cpp: format_ident!("dataChanged"),
                rust: format_ident!("data_changed"),
            },
            safe: true,
            inherit: false,
        }];
        let qobject_idents = create_qobjectname();

        let generated =
            generate_cpp_signals(&signals, &qobject_idents, &ParsedCxxMappings::default()).unwrap();

        assert_eq!(generated.methods.len(), 2);
        let header = if let CppFragment::Header(header) = &generated.methods[0] {
            header
        } else {
            panic!("Expected header")
        };
        assert_str_eq!(
            header,
            "Q_SIGNAL void dataChanged(::std::int32_t trivial, ::std::unique_ptr<QColor> opaque);"
        );

        let (header, source) = if let CppFragment::Pair { header, source } = &generated.methods[1] {
            (header, source)
        } else {
            panic!("Expected Pair")
        };
        assert_str_eq!(
            header,
            "::QMetaObject::Connection dataChangedConnect(::rust::Fn<void(MyObject&, ::std::int32_t trivial, ::std::unique_ptr<QColor> opaque)> func, ::Qt::ConnectionType type);"
        );
        assert_str_eq!(
            source,
            indoc! {r#"
            ::QMetaObject::Connection
            MyObject::dataChangedConnect(::rust::Fn<void(MyObject&, ::std::int32_t trivial, ::std::unique_ptr<QColor> opaque)> func, ::Qt::ConnectionType type)
            {
                return ::QObject::connect(this,
                    &MyObject::dataChanged,
                    this,
                    [&, func = ::std::move(func)](::std::int32_t trivial, ::std::unique_ptr<QColor> opaque) {
                        const ::rust::cxxqtlib1::MaybeLockGuard<MyObject> guard(*this);
                        func(*this, ::std::move(trivial), ::std::move(opaque));
                    },
                    type);
            }
            "#}
        );
    }

    #[test]
    fn test_generate_cpp_signals_mapped_cxx_name() {
        let signals = vec![ParsedSignal {
            method: parse_quote! {
                fn data_changed(self: Pin<&mut MyObject>, mapped: A1);
            },
            qobject_ident: format_ident!("MyObject"),
            mutable: true,
            parameters: vec![ParsedFunctionParameter {
                ident: format_ident!("mapped"),
                ty: parse_quote! { A1 },
            }],
            ident: CombinedIdent {
                cpp: format_ident!("dataChanged"),
                rust: format_ident!("data_changed"),
            },
            safe: true,
            inherit: false,
        }];
        let qobject_idents = create_qobjectname();

        let mut cxx_mappings = ParsedCxxMappings::default();
        cxx_mappings
            .cxx_names
            .insert("A".to_owned(), "A1".to_owned());

        let generated = generate_cpp_signals(&signals, &qobject_idents, &cxx_mappings).unwrap();

        assert_eq!(generated.methods.len(), 2);
        let header = if let CppFragment::Header(header) = &generated.methods[0] {
            header
        } else {
            panic!("Expected header")
        };
        assert_str_eq!(header, "Q_SIGNAL void dataChanged(A1 mapped);");

        let (header, source) = if let CppFragment::Pair { header, source } = &generated.methods[1] {
            (header, source)
        } else {
            panic!("Expected Pair")
        };
        assert_str_eq!(
            header,
            "::QMetaObject::Connection dataChangedConnect(::rust::Fn<void(MyObject&, A1 mapped)> func, ::Qt::ConnectionType type);"
        );
        assert_str_eq!(
            source,
            indoc! {r#"
            ::QMetaObject::Connection
            MyObject::dataChangedConnect(::rust::Fn<void(MyObject&, A1 mapped)> func, ::Qt::ConnectionType type)
            {
                return ::QObject::connect(this,
                    &MyObject::dataChanged,
                    this,
                    [&, func = ::std::move(func)](A1 mapped) {
                        const ::rust::cxxqtlib1::MaybeLockGuard<MyObject> guard(*this);
                        func(*this, ::std::move(mapped));
                    },
                    type);
            }
            "#}
        );
    }

    #[test]
    fn test_generate_cpp_signals_existing_cxx_name() {
        let signals = vec![ParsedSignal {
            method: parse_quote! {
                #[cxx_name = "baseName"]
                fn existing_signal(self: Pin<&mut MyObject>);
            },
            qobject_ident: format_ident!("MyObject"),
            mutable: true,
            parameters: vec![],
            ident: CombinedIdent {
                cpp: format_ident!("baseName"),
                rust: format_ident!("existing_signal"),
            },
            safe: true,
            inherit: true,
        }];
        let qobject_idents = create_qobjectname();

        let generated =
            generate_cpp_signals(&signals, &qobject_idents, &ParsedCxxMappings::default()).unwrap();

        assert_eq!(generated.methods.len(), 1);

        let (header, source) = if let CppFragment::Pair { header, source } = &generated.methods[0] {
            (header, source)
        } else {
            panic!("Expected Pair")
        };
        assert_str_eq!(header, "::QMetaObject::Connection baseNameConnect(::rust::Fn<void(MyObject&)> func, ::Qt::ConnectionType type);");
        assert_str_eq!(
            source,
            indoc! {r#"
            ::QMetaObject::Connection
            MyObject::baseNameConnect(::rust::Fn<void(MyObject&)> func, ::Qt::ConnectionType type)
            {
                return ::QObject::connect(this,
                    &MyObject::baseName,
                    this,
                    [&, func = ::std::move(func)]() {
                        const ::rust::cxxqtlib1::MaybeLockGuard<MyObject> guard(*this);
                        func(*this);
                    },
                    type);
            }
            "#}
        );
    }

    #[test]
    fn test_generate_cpp_signal_free() {
        let signal = ParsedSignal {
            method: parse_quote! {
                fn signal_rust_name(self: Pin<&mut ObjRust>);
            },
            qobject_ident: format_ident!("ObjRust"),
            mutable: true,
            parameters: vec![],
            ident: CombinedIdent {
                cpp: format_ident!("signalRustName"),
                rust: format_ident!("signal_rust_name"),
            },
            safe: true,
            inherit: false,
        };

        let generated = generate_cpp_free_signal(&signal, &ParsedCxxMappings::default()).unwrap();

        let (header, source) = if let CppFragment::Pair { header, source } = &generated {
            (header, source)
        } else {
            panic!("Expected Pair")
        };

        assert_str_eq!(
            header,
            indoc! {
            r#"
            ::QMetaObject::Connection
            ObjRust_signalRustNameConnect(ObjRust& self, ::rust::Fn<void(ObjRust&)> func, ::Qt::ConnectionType type);
            "#}
        );
        assert_str_eq!(
            source,
            indoc! {r#"
            ::QMetaObject::Connection
            ObjRust_signalRustNameConnect(ObjRust& self, ::rust::Fn<void(ObjRust&)> func, ::Qt::ConnectionType type)
            {
                return ::QObject::connect(
                    &self,
                    &ObjRust::signalRustName,
                    &self,
                    [&, func = ::std::move(func)]() {
                        const ::rust::cxxqtlib1::MaybeLockGuard<ObjRust> guard(self);
                        func(self);
                    },
                    type);
            }
            "#}
        );
    }

    #[test]
    fn test_generate_cpp_signal_free_mapped() {
        let signal = ParsedSignal {
            method: parse_quote! {
                #[cxx_name = "signalCxxName"]
                fn signal_rust_name(self: Pin<&mut ObjRust>);
            },
            qobject_ident: format_ident!("ObjRust"),
            mutable: true,
            parameters: vec![],
            ident: CombinedIdent {
                cpp: format_ident!("signalCxxName"),
                rust: format_ident!("signal_rust_name"),
            },
            safe: true,
            inherit: false,
        };

        let mut cxx_mappings = ParsedCxxMappings::default();
        cxx_mappings
            .cxx_names
            .insert("ObjRust".to_owned(), "ObjCpp".to_owned());
        cxx_mappings
            .namespaces
            .insert("ObjRust".to_owned(), "mynamespace".to_owned());

        let generated = generate_cpp_free_signal(&signal, &cxx_mappings).unwrap();

        let (header, source) = if let CppFragment::Pair { header, source } = &generated {
            (header, source)
        } else {
            panic!("Expected Pair")
        };

        assert_str_eq!(
            header,
            indoc! {
            r#"
            ::QMetaObject::Connection
            ObjRust_signalCxxNameConnect(::mynamespace::ObjCpp& self, ::rust::Fn<void(::mynamespace::ObjCpp&)> func, ::Qt::ConnectionType type);
            "#}
        );
        assert_str_eq!(
            source,
            indoc! {r#"
            ::QMetaObject::Connection
            ObjRust_signalCxxNameConnect(::mynamespace::ObjCpp& self, ::rust::Fn<void(::mynamespace::ObjCpp&)> func, ::Qt::ConnectionType type)
            {
                return ::QObject::connect(
                    &self,
                    &::mynamespace::ObjCpp::signalCxxName,
                    &self,
                    [&, func = ::std::move(func)]() {
                        const ::rust::cxxqtlib1::MaybeLockGuard<::mynamespace::ObjCpp> guard(self);
                        func(self);
                    },
                    type);
            }
            "#}
        );
    }
}
