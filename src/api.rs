use super::*;
use reqwest::*;

pub const API: &str = "http://wiki.bibanon.org/api.php";

pub struct MwClient {
    client: Client,
    cookies: HashMap<String, String>,
    pub token: String
}

#[derive(Serialize, Debug)]
pub struct MwArticle {
    pub title: String,
    pub text: String,
    pub summary: String
}

#[derive(Serialize, Debug)]
#[serde(tag = "action")]
#[serde(rename_all = "lowercase")]
enum Action {
    Query { meta: String, #[serde(rename = "type")] type_: Option<String> },
    #[serde(rename = "query")] UiQuery { meta: String, uiprop: String },
    CheckToken { token: String, #[serde(rename = "type")] type_: String },
    Login {  lgname: String, lgpassword: String, lgtoken: String },
    #[serde(rename = "edit")] EditArticle {#[serde(flatten)] article: MwArticle, bot: bool, token: String},
    Upload { filename: String, filepath: PathBuf, token: String }
}

#[derive(Serialize, Debug)]
struct Params {
    #[serde(flatten)]
    action: Action,
    format: String
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Preflight {
    origin: String
}

#[derive(Deserialize, Debug)]
struct LoginToken {
    logintoken: String
}

#[derive(Deserialize, Debug)]
struct CsrfToken {
    csrftoken: String
}

#[derive(Deserialize, Debug)]
struct TokenQuery<T> {
    tokens: T
}

#[derive(Deserialize, Debug)]
pub struct UserData {
    id: i64,
    name: String,
    rights: Vec<String>
}

#[derive(Deserialize, Debug)]
struct UserInfo {
    userinfo: UserData
}

#[derive(Deserialize, Debug)]
pub struct Query<T> {
    query: T
}

#[derive(Deserialize, Debug)]
struct LoginRes {
    result: String
}

#[derive(Deserialize, Debug)]
struct Login {
    login: LoginRes
}

#[derive(Deserialize, Debug)]
struct CheckTokenRes {
    result: String
}

#[derive(Deserialize, Debug)]
struct CheckToken {
    checktoken: CheckTokenRes
}

#[derive(Fail, Deserialize, Debug)]
#[fail(display = "Api Error: {}. {}", code, info)]
pub struct ApiErr {code: String, info: String}

#[derive(Deserialize, Debug)]
pub struct Api<T> {
    batchcomplete: Option<String>,

    #[serde(flatten)]
    res: T
}

impl Action {
    pub fn send(self, client: &Client, headers: header::HeaderMap) -> Res<Response> {
        let params = |x| { Params {action: x, format: "json".to_owned()} };
        let res = match self {
            x @ Action::Query {..} => client.get(API).query(&params(x)).headers(headers).send()?,
            Action::Upload {filename, filepath, token} => {
                let form = multipart::Form::new()
                    .text("action", "upload").file("file", filepath)?
                    .text("filename", filename).text("token", token);

                client.post(API).multipart(form).headers(headers).send()?
            }
            x => client.post(API).form(&params(x)).headers(headers).send()?
        };

        Ok(res)
    }
}

fn get_cookies(vec: &mut HashMap<String, String>, resp: &Response) -> Res<()> {
    for (name, v) in resp.headers().iter() {
        if name == "set-cookie" {
            for set in v.to_str()?.to_owned().split(';') {
                let setv: Vec<&str> = set.split('=').collect();
                let k = setv.get(0).ok_or(format_err!("No cookie key found!"))?;
                let v = setv.get(1).unwrap_or(&"true");
                vec.insert(k.to_owned().trim().to_owned(), v.to_owned().trim().to_owned());
            }
        }
    }

    Ok(())
}

fn parse_token(s: String) -> String {
    s.replace("\\\\", "\\")
}

impl MwClient {
    pub fn new() -> Res<Self> {
        let client = Client::new();
        let mut client = MwClient { client, cookies: HashMap::new(), token: "".to_owned() };

        let parms = Action::Query { meta: "tokens".to_owned(), type_: Some("login".to_owned()) };
        let res = client.do_action::<Query<TokenQuery<LoginToken>>>(parms)?;
        client.token = parse_token(res.query.tokens.logintoken);

        Ok(client)
    }

    fn do_action_req(&mut self, action: Action) -> Res<Response> {
        let mut headers = header::HeaderMap::new();
        let cookies: Vec<String> = self.cookies.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        headers.insert("cookie", header::HeaderValue::from_str(&cookies.join("; "))?);

        let res = self.client.request(Method::OPTIONS, API).query(&Preflight {origin: "*".to_owned()}).send()?;
        get_cookies(&mut self.cookies, &res)?;
        res.error_for_status()?;

        let res = action.send(&self.client, headers)?;

        if let Some(x) = res.headers().get("mediawiki-api-error") {
            return Err(format_err!("Mediawiki Api Error: {}", x.to_str()?));
        }

        get_cookies(&mut self.cookies, &res)?;
        Ok(res.error_for_status()?)
    }

    fn do_action<T: serde::de::DeserializeOwned>(&mut self, action: Action) -> Res<T> {
        let mut res = self.do_action_req(action)?;
        Ok(res.json()?)
    }

    pub fn get_edit_token(&mut self) -> Res<String> {
        let parms = Action::Query {meta: "tokens".to_owned(), type_: None };
        let res = self.do_action::<Query<TokenQuery<CsrfToken>>>(parms)?;

        Ok(parse_token(res.query.tokens.csrftoken))
    }

    pub fn login(&mut self, user: String, pass: String) -> Res<()> {
        let parms = Action::Login { lgtoken: self.token.clone(), lgname: user, lgpassword: pass };
        let l = self.do_action::<Login>(parms)?.login;

        if l.result != "Success" {
            return Err(format_err!("Error logging in: {}", l.result));
        }

        self.token = self.get_edit_token()?;

        Ok(())
    }

    pub fn token_check(&mut self) -> Res<String> {
        let x = self.do_action::<CheckToken>(Action::CheckToken {token: self.token.clone(), type_: "csrf".to_owned()})?;
        Ok(x.checktoken.result)
    }

    pub fn user_info(&mut self) -> Res<UserData> {
        let ui = self.do_action::<Query<UserInfo>>(Action::UiQuery {meta: "userinfo".to_owned(), uiprop: "rights|hasmsg".to_owned()})?;
        Ok(ui.query.userinfo)
    }

    pub fn edit_article(&mut self, a: MwArticle) -> Res<()> {
        let parms = Action::EditArticle {article: a, bot: true, token: self.token.clone()};
        self.do_action_req(parms)?;
        Ok(())
    }

    pub fn upload(&mut self, filename: String, filepath: PathBuf) -> Res<()> {
//        let parms = Action::Upload {filename, filepath, token: self.token.clone()};
//        self.do_action_req(parms)?;
        Ok(())
    }
}
