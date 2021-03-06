use std::time::Duration;

use serde::ser::{Serialize, Serializer};
use spotify::SpotifyHandler;
use tokio::{sync::mpsc::Sender, time::sleep};
use translate::TranslationHandler;
use weather::WeatherHandler;

use crate::{
    bot::SendMsg,
    command_handler::CommandHandlerError,
    db::{DBConn, DBConnError},
    twitch_api::TwitchApi,
};

use self::weather::WeatherError;
pub mod spotify;
mod sys;
pub mod translate;
pub mod weather;

#[derive(Clone, Debug)]
pub enum Action {
    Custom(String),
    AddCmd,
    DelCmd,
    ShowCmd,
    ListCmd,
    Join,
    Part,
}

impl Serialize for Action {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let action = match self {
            Action::Custom(custom) => custom,
            _ => "Unsupported type",
        };

        serializer.serialize_str(action)
    }
}

#[derive(Clone)]
pub struct ActionHandler {
    db_conn: DBConn,
    twitch_api: TwitchApi,
    weather_handler: WeatherHandler,
    spotify_handler: SpotifyHandler,
    translator: TranslationHandler,
}

impl ActionHandler {
    pub fn new(db_conn: DBConn, twitch_api: TwitchApi) -> Self {
        let weather_handler = WeatherHandler::new(db_conn.get_openweathermap_api_key().unwrap());
        let translator = TranslationHandler::new();
        let spotify_handler = SpotifyHandler::new(
            db_conn.get_spotify_cilent_id().unwrap(),
            db_conn.get_spotify_client_secret().unwrap(),
        );

        Self {
            db_conn,
            twitch_api,
            weather_handler,
            translator,
            spotify_handler,
        }
    }

    pub async fn run(
        &self,
        action: &str,
        args: &Vec<String>,
        channel: &str,
        msg_sender: Sender<SendMsg>,
    ) -> Result<Option<String>, CommandHandlerError> {
        println!("Executing action {} with arguments {:?}", action, args);

        match action {
            "spotify" => Ok(Some(self.get_spotify(channel).await?)),
            "spotify.playlist" => Ok(Some(self.get_spotify_playlist(channel).await?)),
            "lastsong" => Ok(Some(self.get_spotify_last_song(channel).await?)),
            "hitman" => Ok(match args.first() {
                Some(name) => Some(
                    self.hitman(channel, &name.replace('@', ""), msg_sender)
                        .await?,
                ),
                None => Some(String::from("user not specified")),
            }),
            "bodyguard" => Ok(match args.first() {
                Some(name) => Some(
                    self.bodyguard(channel, &name.replace('@', ""), msg_sender)
                        .await?,
                ),
                None => Some(String::from("user not specified")),
            }),
            "ping" => Ok(Some(sys::SysInfo::ping())),
            "commercial" => Ok(Some(
                self.run_ad(channel, args.first().unwrap().parse().unwrap())
                    .await?,
            )),
            "weather" => Ok(match args.is_empty() {
                false => Some(self.get_weather(&args.join(" ")).await?),
                true => Some(String::from("location not specified")),
            }),
            "translate" => Ok(Some(self.translate(args.first().unwrap()).await?)),
            "emoteonly" => match args.first().unwrap().parse::<u64>() {
                Ok(duration) => {
                    self.emote_only(channel, duration, msg_sender).await;
                    Ok(None)
                }
                Err(_) => Err(CommandHandlerError::ExecutionError(String::from(
                    "invalid duration",
                ))),
            },
            "increment" => match args.first() {
                Some(username) => {
                    self.db_conn.increment_currency(username)?;
                    Ok(None)
                }
                None => Err(CommandHandlerError::ExecutionError(
                    "Missing username".to_string(),
                )),
            },
            "currency" => match args.first() {
                Some(username) => Ok(Some(self.db_conn.get_currency(username)?.to_string())),
                None => Err(CommandHandlerError::ExecutionError(
                    "Missing username".to_string(),
                )),
            },
            _ => Err(CommandHandlerError::ExecutionError(format!(
                "unknown action {}",
                action
            ))),
        }
    }

    async fn get_spotify(&self, channel: &str) -> Result<String, CommandHandlerError> {
        match self.db_conn.get_spotify_access_token(channel) {
            Ok((access_token, _)) => {
                match self.spotify_handler.get_current_song(&access_token).await? {
                    Some(song) => Ok(song),
                    None => Ok(String::from("no song is currently playing")),
                }
            }
            Err(e) => match e {
                DBConnError::NotFound => Ok(String::from("not configured for this channel")),
                _ => Err(CommandHandlerError::DBError(e)),
            },
        }
    }

    async fn get_spotify_playlist(&self, channel: &str) -> Result<String, CommandHandlerError> {
        match self.db_conn.get_spotify_access_token(channel) {
            Ok((access_token, _)) => {
                match self
                    .spotify_handler
                    .get_current_playlist(&access_token)
                    .await?
                {
                    Some(playlist) => Ok(playlist),
                    None => Ok(String::from("not currently playing a playlist")),
                }
            }
            Err(e) => match e {
                DBConnError::NotFound => Ok(String::from("not configured for this channel")),
                _ => Err(CommandHandlerError::DBError(e)),
            },
        }
    }

    async fn get_spotify_last_song(&self, channel: &str) -> Result<String, CommandHandlerError> {
        match self.db_conn.get_spotify_access_token(channel) {
            Ok((access_token, _)) => {
                match self
                    .spotify_handler
                    .get_recently_played(&access_token)
                    .await
                {
                    Ok(recently_played) => Ok(recently_played),
                    Err(e) => Ok(format!("error getting last song: {:?}", e)),
                }
            }
            Err(e) => match e {
                DBConnError::NotFound => Ok(String::from("not configured for this channel")),
                _ => Err(CommandHandlerError::DBError(e)),
            },
        }
    }

    async fn hitman(
        &self,
        channel: &str,
        user: &str,
        msg_sender: Sender<SendMsg>,
    ) -> Result<String, CommandHandlerError> {
        self.db_conn.add_hitman(channel, user)?;

        msg_sender
            .send(SendMsg::Say((
                channel.to_owned(),
                format!("Timing out {} in 15 seconds...", user),
            )))
            .await
            .expect("Failed to send");

        sleep(Duration::from_secs(15)).await;

        match self.db_conn.get_hitman_protected(user, channel)? {
            false => {
                msg_sender
                    .send(SendMsg::Raw((
                        channel.to_owned(),
                        format!("/timeout {} 600", user),
                    )))
                    .await
                    .expect("Failed to send");

                Ok(format!("{} timed out for 10 minutes!", user))
            }
            true => {
                self.db_conn.set_hitman_protection(user, channel, &false)?;

                Ok(String::new())
            }
        }
    }

    async fn bodyguard(
        &self,
        channel: &str,
        user: &str,
        msg_sender: Sender<SendMsg>,
    ) -> Result<String, CommandHandlerError> {
        self.db_conn.set_hitman_protection(user, channel, &true)?;

        msg_sender
            .send(SendMsg::Say((
                channel.to_owned(),
                format!("{} has been guarded!", user),
            )))
            .await
            .expect("Failed to send");

        Ok(String::new())
    }

    async fn emote_only(&self, channel: &str, duration: u64, msg_sender: Sender<SendMsg>) {
        msg_sender
            .send(SendMsg::Say((
                channel.to_string(),
                format!("Emote-only enabled for {} seconds!", duration),
            )))
            .await
            .unwrap();

        msg_sender
            .send(SendMsg::Raw((
                channel.to_string(),
                "/emoteonly".to_string(),
            )))
            .await
            .unwrap();

        sleep(Duration::from_secs(duration)).await;

        msg_sender
            .send(SendMsg::Raw((
                channel.to_string(),
                "/emoteonlyoff".to_string(),
            )))
            .await
            .unwrap();
    }

    async fn run_ad(&self, channel: &str, duration: u8) -> Result<String, CommandHandlerError> {
        if duration == 60 || duration == 120 || duration == 180 {
            self.twitch_api.run_ad(channel, duration).await?;
            Ok(format!("Running an ad for {} seconds", duration))
        } else {
            Ok(String::from("Invalid ad duration"))
        }
    }

    async fn get_weather(&self, location: &str) -> Result<String, CommandHandlerError> {
        match self.weather_handler.get_weather(location.to_owned()).await {
            Ok(weather) => Ok(format!(
                "{}, {}: {}°C, {}",
                weather.name,
                weather.sys.country.unwrap_or_default(),
                weather.main.temp,
                weather.weather.first().unwrap().description
            )),
            Err(e) => match e {
                WeatherError::InvalidLocation => Ok(String::from("location not found")),
                _ => Ok(format!("Failed getting weather: {:?}", e)),
            },
        }
    }

    async fn translate(&self, text: &str) -> Result<String, CommandHandlerError> {
        match self.translator.translate(text).await {
            Ok(translation) => Ok(format!(
                "{} -> {}: {}",
                translation.src, translation.dest, translation.text
            )),
            Err(e) => Ok(format!("error when translating: {:?}", e)),
        }
    }
}
