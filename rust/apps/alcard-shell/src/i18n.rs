//! Shell labels. Integrate with shared alcard-l10n during the localization stage.
use crate::{
    controls::{Availability, Control, Failure as ControlFailure, Reading},
    model::{
        Action, ApplicationOption, InfoKind, LaunchPhase, NotificationOption, Screen,
        WindowOperation,
    },
    radios::{self, Availability as RadioAvailability, Failure as RadioFailure, Kind as RadioKind},
    sleep::{MonitorState, Snapshot as SleepSnapshot, Stage as SleepStage},
    status::{BatteryState, ChargeState, LinkState},
    system::{Capability, Failure, Operation},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    Ru,
    En,
    Es,
}

impl Locale {
    pub fn detect() -> Self {
        let lang = std::env::var("LC_ALL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("LANG").ok())
            .unwrap_or_default()
            .to_lowercase();
        if lang.starts_with("ru") {
            Self::Ru
        } else if lang.starts_with("es") {
            Self::Es
        } else {
            Self::En
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
            Self::Es => "es",
        }
    }
    pub fn screen(self, screen: Screen) -> &'static str {
        match (self, screen) {
            (Self::Ru, Screen::Home) => "Главная",
            (Self::Ru, Screen::Recent) => "Недавние",
            (Self::Ru, Screen::Notifications) => "Уведомления",
            (Self::Ru, Screen::Menu) => "Приложения",
            (Self::Es, Screen::Home) => "Inicio",
            (Self::Es, Screen::Recent) => "Recientes",
            (Self::Es, Screen::Notifications) => "Avisos",
            (Self::Es, Screen::Menu) => "Aplicaciones",
            (_, Screen::Home) => "Home",
            (_, Screen::Recent) => "Recent",
            (_, Screen::Notifications) => "Notifications",
            (_, Screen::Menu) => "Applications",
        }
    }
    pub fn action(self, action: Action) -> &'static str {
        match (self, action) {
            (_, Action::None) => "",
            (_, Action::Acknowledge) => "OK",
            (_, Action::ApplyText) => "Enter",
            (_, Action::CancelText) => "Esc",
            (Self::Ru, Action::Mute) => "Выкл.",
            (Self::Ru, Action::Unmute) => "Вкл.",
            (Self::Es, Action::Mute) => "Sil.",
            (Self::Es, Action::Unmute) => "Sonido",
            (_, Action::Mute) => "Mute",
            (_, Action::Unmute) => "Unmute",
            (Self::Ru, Action::TurnOn) => "Вкл.",
            (Self::Ru, Action::TurnOff) => "Выкл.",
            (Self::Es, Action::TurnOn) => "Activ.",
            (Self::Es, Action::TurnOff) => "Desact",
            (_, Action::TurnOn) => "On",
            (_, Action::TurnOff) => "Off",
            (Self::Ru, Action::Quick) => "Быстро",
            (Self::Es, Action::Quick) => "Rápido",
            (_, Action::Quick) => "Quick",
            (Self::Ru, Action::Search) => "Поиск",
            (Self::Ru, Action::Clear) => "Очист.",
            (Self::Ru, Action::Retry) => "Повтор",
            (Self::Es, Action::Search) => "Buscar",
            (Self::Es, Action::Clear) => "Borrar",
            (Self::Es, Action::Retry) => "Reint.",
            (_, Action::Search) => "Search",
            (_, Action::Clear) => "Clear",
            (_, Action::Retry) => "Retry",
            (Self::Ru, Action::Recent) => "Недав.",
            (Self::Ru, Action::Menu) => "Меню",
            (Self::Ru, Action::Notifications) => "Увед.",
            (Self::Ru, Action::Open) => "Откр.",
            (Self::Ru, Action::Launch) => "Запуск",
            (Self::Ru, Action::Back) => "Назад",
            (Self::Ru, Action::Confirm) => "Да",
            (Self::Ru, Action::Cancel) => "Отмена",
            (Self::Ru, Action::Decline) => "Нет",
            (Self::Ru, Action::Read) => "Читать",
            (Self::Ru, Action::Actions) => "Опции",
            (Self::Ru, Action::Select) => "Выбор",
            (Self::Es, Action::Recent) => "Últim.",
            (Self::Es, Action::Menu) => "Menú",
            (Self::Es, Action::Notifications) => "Avisos",
            (Self::Es, Action::Open) => "Abrir",
            (Self::Es, Action::Launch) => "Lanzar",
            (Self::Es, Action::Back) => "Volver",
            (Self::Es, Action::Confirm) => "Sí",
            (Self::Es, Action::Cancel) => "Anular",
            (Self::Es, Action::Read) => "Leer",
            (Self::Es, Action::Actions) => "Opc.",
            (Self::Es, Action::Select) => "Elegir",
            (_, Action::Recent) => "Recent",
            (_, Action::Menu) => "Menu",
            (_, Action::Notifications) => "Notifs",
            (_, Action::Open) => "Open",
            (_, Action::Launch) => "Launch",
            (_, Action::Back) => "Back",
            (_, Action::Confirm) => "Yes",
            (_, Action::Cancel) => "Cancel",
            (_, Action::Decline) => "No",
            (_, Action::Read) => "Read",
            (_, Action::Actions) => "Opts",
            (_, Action::Select) => "Select",
        }
    }
    pub fn quick_title(self) -> &'static str {
        match self {
            Self::Ru => "Быстрые настройки",
            Self::En => "Quick settings",
            Self::Es => "Ajustes rápidos",
        }
    }
    pub fn lock_title(self) -> &'static str {
        match self {
            Self::Ru => "Заблокировать",
            Self::En => "Lock screen",
            Self::Es => "Bloquear",
        }
    }
    pub fn suspend_title(self) -> &'static str {
        match self {
            Self::Ru => "Перейти в сон",
            Self::En => "Suspend",
            Self::Es => "Suspender",
        }
    }
    pub fn suspend_confirm(self) -> &'static str {
        match self {
            Self::Ru => "Перейти в спящий режим?",
            Self::En => "Suspend the device?",
            Self::Es => "¿Suspender el dispositivo?",
        }
    }
    pub fn suspend_warning(self) -> &'static str {
        match self {
            Self::Ru => "Сначала экран будет заблокирован, затем система получит запрос сна. После пробуждения потребуется пароль.",
            Self::En => "Shell will lock the screen before requesting sleep. Use your password to unlock after waking.",
            Self::Es => "Shell bloqueará la pantalla antes de solicitar el reposo. Tras reactivarlo, use su contraseña para desbloquear.",
        }
    }
    pub fn suspend_hint(self, snapshot: SleepSnapshot) -> &'static str {
        match (self, snapshot.monitor, snapshot.capability) {
            (Self::Ru, MonitorState::Preparing, _) => "Система готовится ко сну",
            (Self::Es, MonitorState::Preparing, _) => "Sistema preparando el reposo",
            (_, MonitorState::Preparing, _) => "System preparing to sleep",
            (_, MonitorState::Connecting, _) => self.system_capability(Capability::Checking),
            (_, MonitorState::Unavailable, _) => self.system_capability(Capability::Unavailable),
            (Self::Ru, MonitorState::Ready, Capability::Allowed) => "Блокировка, затем сон",
            (Self::Es, MonitorState::Ready, Capability::Allowed) => "Bloquear y suspender",
            (_, MonitorState::Ready, Capability::Allowed) => "Lock, then sleep",
            (Self::Ru, MonitorState::Ready, Capability::Authentication) => {
                "Нужны права; отсюда недоступно"
            }
            (Self::Es, MonitorState::Ready, Capability::Authentication) => {
                "Requiere permiso; no disponible"
            }
            (_, MonitorState::Ready, Capability::Authentication) => {
                "Needs authorization; unavailable"
            }
            (_, MonitorState::Ready, capability) => self.system_capability(capability),
        }
    }
    pub fn suspend_unavailable(self, snapshot: SleepSnapshot) -> &'static str {
        match (self, snapshot.monitor, snapshot.capability) {
            (_, MonitorState::Preparing, _) => self.suspend_external(),
            (Self::Ru, MonitorState::Connecting, _) => "Проверяем доступность сна. Повторите проверку, если ожидание затянулось.",
            (Self::Es, MonitorState::Connecting, _) => "Comprobando si el reposo está disponible. Repita la comprobación si tarda demasiado.",
            (_, MonitorState::Connecting, _) => "Checking sleep availability. Retry the check if it takes too long.",
            (_, MonitorState::Unavailable, _) => self.suspend_failure(Failure::Unavailable),
            (Self::Ru, MonitorState::Ready, Capability::Allowed) => "Сон доступен. Вернитесь к быстрым действиям, чтобы подтвердить новый запрос.",
            (Self::Es, MonitorState::Ready, Capability::Allowed) => "El reposo está disponible. Vuelva a Acciones rápidas para confirmar una nueva solicitud.",
            (_, MonitorState::Ready, Capability::Allowed) => "Sleep is available. Return to Quick settings to confirm a new request.",
            (Self::Ru, MonitorState::Ready, Capability::Authentication) => "Нужны дополнительные права. Эта кнопка не запрашивает пароль после блокировки, поэтому сон отсюда недоступен.",
            (Self::Es, MonitorState::Ready, Capability::Authentication) => "Se necesita autorización. Esta acción no pide una contraseña tras bloquear la pantalla; el reposo no está disponible aquí.",
            (_, MonitorState::Ready, Capability::Authentication) => "Authorization is required. This action cannot ask for a password after locking, so sleep is unavailable here.",
            (Self::Ru, MonitorState::Ready, Capability::Unsupported) => "Сон не поддерживается этой системой.",
            (Self::Es, MonitorState::Ready, Capability::Unsupported) => "Este sistema no admite el reposo.",
            (_, MonitorState::Ready, Capability::Unsupported) => "This system does not support sleep.",
            (_, MonitorState::Ready, Capability::Checking) => self.system_capability(Capability::Checking),
            (_, MonitorState::Ready, Capability::Denied) => self.suspend_failure(Failure::Denied),
            (_, MonitorState::Ready, Capability::Blocked) => self.suspend_failure(Failure::Blocked),
            (_, MonitorState::Ready, Capability::Unavailable) => self.suspend_failure(Failure::Unavailable),
        }
    }
    pub fn suspend_external(self) -> &'static str {
        match self {
            Self::Ru => "Система готовится ко сну. Shell ожидает блокировку; внешний запрос сна может продолжиться без неё.",
            Self::En => "The system is preparing to sleep. Shell is waiting for the lock; an external sleep request can proceed without it.",
            Self::Es => "El sistema prepara el reposo. Shell espera el bloqueo; una solicitud externa de reposo puede continuar sin él.",
        }
    }
    pub fn suspend_status(self, stage: SleepStage) -> &'static str {
        match (self, stage) {
            (Self::Ru, SleepStage::Preparing) => "Проверяем, разрешён ли сон. Отмена доступна, пока запрос не передан системе.",
            (Self::Es, SleepStage::Preparing) => "Comprobando si se permite el reposo. Puede cancelar antes de enviar la solicitud al sistema.",
            (_, SleepStage::Preparing) => "Checking permission to sleep. You can cancel before the request is sent to the system.",
            (Self::Ru, SleepStage::Locking) => "Ожидаем подтверждения блокировки. Сон будет запрошен только после блокировки экрана.",
            (Self::Es, SleepStage::Locking) => "Esperando la confirmación del bloqueo. Solo se solicitará el reposo después de bloquear la pantalla.",
            (_, SleepStage::Locking) => "Waiting for the lock to be confirmed. Sleep will only be requested after the screen is locked.",
            (Self::Ru, SleepStage::Queued) => "Блокировка подтверждена. Передаём запрос системе. Закрытие карточки не отменяет его.",
            (Self::Es, SleepStage::Queued) => "Bloqueo confirmado. Enviando la solicitud al sistema. Cerrar esta tarjeta no la cancela.",
            (_, SleepStage::Queued) => "The lock is confirmed. Sending the request to the system. Closing this card does not cancel it.",
            (Self::Ru, SleepStage::Requested) => "Запрос передан системе. Окончание перехода ещё не подтверждено. Закрытие карточки не отменяет запрос.",
            (Self::Es, SleepStage::Requested) => "Solicitud enviada al sistema. Aún no se ha confirmado el fin de la transición. Cerrar esta tarjeta no la cancela.",
            (_, SleepStage::Requested) => "The request was sent to the system. The transition has not yet ended. Closing this card does not cancel the request.",
            (Self::Ru, SleepStage::Ended) => "Переход ко сну завершён. Сам сон не подтверждён. Автоматической разблокировки нет.",
            (Self::Es, SleepStage::Ended) => "Terminó la transición al reposo. Esto no confirma que el equipo durmiera. La sesión no se desbloquea automáticamente.",
            (_, SleepStage::Ended) => "The sleep transition has ended. This does not confirm the device slept. The session is not unlocked automatically.",
            (Self::Ru, SleepStage::Cancelled) => "Подготовка ко сну отменена. Запрос сна не отправлен. Уже начатая блокировка остаётся.",
            (Self::Es, SleepStage::Cancelled) => "Se canceló la preparación del reposo. No se envió la solicitud. Un bloqueo ya iniciado se mantiene.",
            (_, SleepStage::Cancelled) => "Sleep preparation was cancelled. No sleep request was sent. Any lock already started remains in place.",
            (_, SleepStage::Unknown | SleepStage::Failed) => self.suspend_failure(Failure::Unknown),
        }
    }
    pub fn suspend_failure(self, failure: Failure) -> &'static str {
        match (self, failure) {
            (Self::Ru, Failure::Unavailable) => "Служба сна недоступна. Повторите проверку позже.",
            (Self::Es, Failure::Unavailable) => "El servicio de reposo no está disponible. Vuelva a comprobarlo más tarde.",
            (_, Failure::Unavailable) => "The sleep service is unavailable. Check again later.",
            (Self::Ru, Failure::Denied) => "Система не разрешила переход в сон.",
            (Self::Es, Failure::Denied) => "El sistema no permitió el reposo.",
            (_, Failure::Denied) => "The system did not permit sleep.",
            (Self::Ru, Failure::Blocked) => "Сон заблокирован системой или другой программой. Завершите текущую работу и повторите проверку.",
            (Self::Es, Failure::Blocked) => "El sistema u otra aplicación bloquea el reposo. Termine el trabajo en curso y vuelva a comprobarlo.",
            (_, Failure::Blocked) => "The system or another application is blocking sleep. Finish the work in progress and check again.",
            (Self::Ru, Failure::SessionUnavailable) => "Не удалось подтвердить текущий графический сеанс. Запрос сна не отправлен.",
            (Self::Es, Failure::SessionUnavailable) => "No se pudo verificar la sesión gráfica actual. No se envió la solicitud de reposo.",
            (_, Failure::SessionUnavailable) => "The current graphical session could not be verified. No sleep request was sent.",
            (Self::Ru, Failure::Timeout) => "Подготовка ко сну не завершилась вовремя. Запрос сна не отправлен.",
            (Self::Es, Failure::Timeout) => "La preparación del reposo no terminó a tiempo. No se envió la solicitud de reposo.",
            (_, Failure::Timeout) => "Sleep preparation did not finish in time. No sleep request was sent.",
            (Self::Ru, Failure::Unknown) => "Результат запроса сна неизвестен. Автоматического повтора нет. Закрытие карточки не отменяет запрос.",
            (Self::Es, Failure::Unknown) => "Se desconoce el resultado de la solicitud de reposo. No se repite automáticamente. Cerrar esta tarjeta no la cancela.",
            (_, Failure::Unknown) => "The sleep request outcome is unknown. It is not retried automatically. Closing this card does not cancel the request.",
            (_, Failure::Cancelled) => self.suspend_status(SleepStage::Cancelled),
        }
    }
    pub fn control_title(self, control: Control) -> &'static str {
        match (self, control) {
            (Self::Ru, Control::Brightness) => "Яркость",
            (Self::Ru, Control::Volume) => "Громкость",
            (Self::Es, Control::Brightness) => "Brillo",
            (Self::Es, Control::Volume) => "Volumen",
            (_, Control::Brightness) => "Brightness",
            (_, Control::Volume) => "Volume",
        }
    }
    pub fn control_hint(self, control: Control, reading: Reading) -> &'static str {
        match (self, reading.availability) {
            (Self::Ru, Availability::Checking) => "Проверка устройства…",
            (Self::Es, Availability::Checking) => "Comprobando el dispositivo…",
            (_, Availability::Checking) => "Checking device…",
            (Self::Ru, Availability::Unavailable) => "Устройство недоступно",
            (Self::Es, Availability::Unavailable) => "Dispositivo no disponible",
            (_, Availability::Unavailable) => "Device unavailable",
            (Self::Ru, Availability::Denied) => "Нет разрешения на изменение",
            (Self::Es, Availability::Denied) => "Sin permiso para cambiar",
            (_, Availability::Denied) => "No permission to change",
            (_, Availability::Available) => {
                if reading.above_safe_limit {
                    if control == Control::Volume && reading.muted == Some(true) {
                        return match self {
                            Self::Ru => "Без звука; сначала уменьшите",
                            Self::Es => "Silenciado; baje para activar",
                            Self::En => "Muted; lower the level to unmute",
                        };
                    }
                    return match self {
                        Self::Ru => "Выше 100%; повышение недоступно",
                        Self::Es => "Más del 100%; no se puede subir",
                        Self::En => "Above 100%; cannot increase",
                    };
                }
                if control == Control::Volume {
                    return match (self, reading.muted) {
                        (Self::Ru, Some(true)) => "Звук выключен",
                        (Self::Es, Some(true)) => "Sonido silenciado",
                        (_, Some(true)) => "Muted",
                        (Self::Ru, Some(false)) => "Звук включён",
                        (Self::Es, Some(false)) => "Sonido activado",
                        (_, Some(false)) => "Unmuted",
                        (Self::Ru, None) => "Отключение звука недоступно",
                        (Self::Es, None) => "Silenciar no está disponible",
                        (_, None) => "Mute is unavailable",
                    };
                }
                match self {
                    Self::Ru => "Минимальная яркость 1%",
                    Self::Es => "Brillo mínimo del 1%",
                    Self::En => "Minimum brightness is 1%",
                }
            }
        }
    }
    pub fn controls_adjust_hint(self) -> &'static str {
        match self {
            Self::Ru => "Влево/вправо: шаг 5%",
            Self::Es => "Izq./der.: pasos del 5%",
            Self::En => "Left/Right: steps of 5%",
        }
    }
    pub fn controls_pending(self, refreshing: bool) -> &'static str {
        match (self, refreshing) {
            (Self::Ru, true) => "Проверка устройств…",
            (Self::Ru, false) => "Изменение и проверка значения…",
            (Self::Es, true) => "Comprobando los dispositivos…",
            (Self::Es, false) => "Cambiando y comprobando el valor…",
            (_, true) => "Checking devices…",
            (_, false) => "Changing and checking the value…",
        }
    }
    pub fn controls_failure(self, failure: ControlFailure) -> &'static str {
        match (self, failure) {
            (Self::Ru, ControlFailure::Unavailable) => "Устройство недоступно",
            (Self::Ru, ControlFailure::Denied) => "Нет разрешения на изменение",
            (Self::Ru, ControlFailure::Timeout) => "Нет ответа. Проверьте значение.",
            (Self::Ru, ControlFailure::Unknown) => "Результат не подтверждён",
            (Self::Ru, ControlFailure::Changed) => "Устройство изменилось",
            (Self::Ru, ControlFailure::Invalid) => "Неверный ответ устройства",
            (Self::Es, ControlFailure::Unavailable) => "Dispositivo no disponible",
            (Self::Es, ControlFailure::Denied) => "Sin permiso para cambiar",
            (Self::Es, ControlFailure::Timeout) => "Sin respuesta. Compruebe el valor.",
            (Self::Es, ControlFailure::Unknown) => "Resultado sin confirmar",
            (Self::Es, ControlFailure::Changed) => "El dispositivo ha cambiado",
            (Self::Es, ControlFailure::Invalid) => "Respuesta del dispositivo no válida",
            (_, ControlFailure::Unavailable) => "Device unavailable",
            (_, ControlFailure::Denied) => "No permission to change",
            (_, ControlFailure::Timeout) => "No response. Check the value.",
            (_, ControlFailure::Unknown) => "Result not confirmed",
            (_, ControlFailure::Changed) => "Device changed",
            (_, ControlFailure::Invalid) => "Invalid device response",
        }
    }
    pub fn radio_title(self, kind: RadioKind) -> &'static str {
        match kind {
            RadioKind::Wifi => "Wi-Fi",
            RadioKind::Bluetooth => "Bluetooth",
        }
    }
    pub fn radio_list_title(self, kind: RadioKind) -> &'static str {
        match (self, kind) {
            (Self::Ru, RadioKind::Wifi) => "Сети Wi-Fi",
            (Self::Ru, RadioKind::Bluetooth) => "Устройства Bluetooth",
            (Self::Es, RadioKind::Wifi) => "Redes Wi-Fi",
            (Self::Es, RadioKind::Bluetooth) => "Dispositivos Bluetooth",
            (_, RadioKind::Wifi) => "Wi-Fi networks",
            (_, RadioKind::Bluetooth) => "Bluetooth devices",
        }
    }
    pub fn radio_value(self, state: &radios::State) -> &'static str {
        if state.availability != RadioAvailability::Available {
            return "--";
        }
        match (self, state.powered) {
            (Self::Ru, Some(true)) => "Вкл.",
            (Self::Ru, Some(false)) => "Выкл.",
            (Self::Es, Some(true)) => "Activo",
            (Self::Es, Some(false)) => "Inact.",
            (_, Some(true)) => "On",
            (_, Some(false)) => "Off",
            (_, None) => "--",
        }
    }
    pub fn radio_state(self, state: &radios::State) -> &'static str {
        match (self, state.availability) {
            (Self::Ru, RadioAvailability::Checking) => "Проверка радио…",
            (Self::Es, RadioAvailability::Checking) => "Comprobando la radio…",
            (_, RadioAvailability::Checking) => "Checking radio…",
            (Self::Ru, RadioAvailability::Absent) => "Нет адаптера",
            (Self::Es, RadioAvailability::Absent) => "No hay adaptador",
            (_, RadioAvailability::Absent) => "No adapter",
            (Self::Ru, RadioAvailability::Unavailable) => "Радио недоступно",
            (Self::Es, RadioAvailability::Unavailable) => "Radio no disponible",
            (_, RadioAvailability::Unavailable) => "Radio unavailable",
            (Self::Ru, RadioAvailability::Blocked) => "Аппаратная блокировка",
            (Self::Es, RadioAvailability::Blocked) => "Bloqueo de hardware",
            (_, RadioAvailability::Blocked) => "Hardware blocked",
            (_, RadioAvailability::Available) => match (self, state.powered) {
                (Self::Ru, Some(true)) => "Включено",
                (Self::Ru, Some(false)) => "Выключено",
                (Self::Ru, None) => "Состояние не подтверждено",
                (Self::Es, Some(true)) => "Activada",
                (Self::Es, Some(false)) => "Desactivada",
                (Self::Es, None) => "Estado sin confirmar",
                (_, Some(true)) => "Enabled",
                (_, Some(false)) => "Disabled",
                (_, None) => "State not confirmed",
            },
        }
    }
    pub fn radio_pending(self, operation: radios::Operation) -> &'static str {
        match (self, operation) {
            (Self::Ru, radios::Operation::Refresh) => "Проверка радио…",
            (Self::Es, radios::Operation::Refresh) => "Comprobando la radio…",
            (_, radios::Operation::Refresh) => "Checking radio…",
            (Self::Ru, radios::Operation::List { .. }) => "Обновление списка…",
            (Self::Es, radios::Operation::List { .. }) => "Actualizando la lista…",
            (_, radios::Operation::List { .. }) => "Refreshing list…",
            (Self::Ru, radios::Operation::SetPowered { powered: true, .. }) => "Включение…",
            (Self::Ru, radios::Operation::SetPowered { powered: false, .. }) => "Выключение…",
            (Self::Es, radios::Operation::SetPowered { powered: true, .. }) => "Activando…",
            (Self::Es, radios::Operation::SetPowered { powered: false, .. }) => "Desactivando…",
            (_, radios::Operation::SetPowered { powered: true, .. }) => "Turning on…",
            (_, radios::Operation::SetPowered { powered: false, .. }) => "Turning off…",
        }
    }
    pub fn radio_failure(self, failure: RadioFailure) -> &'static str {
        match (self, failure) {
            (Self::Ru, RadioFailure::Unavailable) => "Радио недоступно",
            (Self::Es, RadioFailure::Unavailable) => "Radio no disponible",
            (_, RadioFailure::Unavailable) => "Radio unavailable",
            (Self::Ru, RadioFailure::Permission) => "Нет разрешения на изменение",
            (Self::Es, RadioFailure::Permission) => "Sin permiso para cambiar",
            (_, RadioFailure::Permission) => "No permission to change",
            (Self::Ru, RadioFailure::Timeout) => "Нет ответа. Проверьте состояние.",
            (Self::Es, RadioFailure::Timeout) => "Sin respuesta. Compruebe el estado.",
            (_, RadioFailure::Timeout) => "No response. Check the state.",
            (Self::Ru, RadioFailure::Failed) => "Не удалось выполнить запрос",
            (Self::Es, RadioFailure::Failed) => "No se pudo completar la solicitud",
            (_, RadioFailure::Failed) => "The request failed",
            (Self::Ru, RadioFailure::InvalidResponse) => "Состояние не подтверждено",
            (Self::Es, RadioFailure::InvalidResponse) => "Estado sin confirmar",
            (_, RadioFailure::InvalidResponse) => "State not confirmed",
        }
    }
    pub fn radio_empty(self, kind: RadioKind) -> &'static str {
        match (self, kind) {
            (Self::Ru, RadioKind::Wifi) => "Сети не найдены",
            (Self::Ru, RadioKind::Bluetooth) => "Устройства не найдены",
            (Self::Es, RadioKind::Wifi) => "No se encontraron redes",
            (Self::Es, RadioKind::Bluetooth) => "No se encontraron dispositivos",
            (_, RadioKind::Wifi) => "No networks found",
            (_, RadioKind::Bluetooth) => "No devices found",
        }
    }
    pub fn radio_list_hint(self) -> &'static str {
        match self {
            Self::Ru => "4: сведения; 7: обновить",
            Self::Es => "4: detalles; 7: actualizar",
            Self::En => "4: details; 7: refresh",
        }
    }
    pub fn radio_details_title(self, kind: RadioKind) -> &'static str {
        match (self, kind) {
            (Self::Ru, RadioKind::Wifi) => "Сеть Wi-Fi",
            (Self::Ru, RadioKind::Bluetooth) => "Устройство Bluetooth",
            (Self::Es, RadioKind::Wifi) => "Red Wi-Fi",
            (Self::Es, RadioKind::Bluetooth) => "Dispositivo Bluetooth",
            (_, RadioKind::Wifi) => "Wi-Fi network",
            (_, RadioKind::Bluetooth) => "Bluetooth device",
        }
    }
    pub fn radio_details_text(self, entry: &radios::Entry) -> String {
        // These are display strings from untrusted network/device names. They
        // never become terminal escapes or commands, and the full bounded ID
        // remains readable in the scrolled details even when a row is shortened.
        let clean = |value: &str| -> String {
            value
                .chars()
                .filter(|ch| !ch.is_control() || *ch == '\n')
                .take(4096)
                .collect()
        };
        let name = match self {
            Self::Ru => "Имя",
            Self::Es => "Nombre",
            Self::En => "Name",
        };
        format!(
            "{name}: {}\nID: {}\n\n{}",
            clean(&entry.name),
            clean(&entry.id),
            clean(&entry.detail)
        )
    }
    pub fn radio_details_lines(self, entry: &radios::Entry) -> Vec<String> {
        crate::model::wrap_text(&self.radio_details_text(entry), crate::modal::MODAL_COLUMNS)
    }
    pub fn lock_hint(self) -> &'static str {
        match self {
            Self::Ru => "Пароль текущего пользователя",
            Self::En => "Current user's password",
            Self::Es => "Contraseña del usuario",
        }
    }
    pub fn lock_status(self, phase: crate::locking::Phase) -> &'static str {
        use crate::locking::Phase;
        match (self, phase) {
            (Self::Ru, Phase::Starting) => "Ожидаем подтверждение блокировки. Пароль вводится на отдельном экране.",
            (Self::En, Phase::Starting) => "Waiting for the compositor to confirm the lock. Enter your password on the separate lock screen.",
            (Self::Es, Phase::Starting) => "Esperando la confirmación del bloqueo. La contraseña se introduce en la pantalla de bloqueo.",
            (Self::Ru, Phase::Locked) => "Сеанс заблокирован. Разблокировка выполняется на экране блокировки.",
            (Self::En, Phase::Locked) => "The session is locked. Use the lock screen to unlock it.",
            (Self::Es, Phase::Locked) => "La sesión está bloqueada. Use la pantalla de bloqueo para desbloquearla.",
            (Self::Ru, Phase::Failed) => "Не удалось подтвердить блокировку. Проверьте установку компонента alcard-lock, PAM и поддержку блокировки в композиторе.",
            (Self::En, Phase::Failed) => "Could not confirm the lock. Check that alcard-lock, its PAM service and compositor session locking are installed.",
            (Self::Es, Phase::Failed) => "No se pudo confirmar el bloqueo. Revise alcard-lock, su servicio PAM y el soporte del compositor.",
            (Self::Ru, Phase::Uncertain) => "Состояние блокировки не подтверждено. Повторный запрос не отправлен. Закрытие этой карточки не разблокирует сеанс.",
            (Self::En, Phase::Uncertain) => "The lock state is unconfirmed. No second request was sent. Closing this card does not unlock the session.",
            (Self::Es, Phase::Uncertain) => "No se ha confirmado el estado del bloqueo. No se repitió la solicitud. Cerrar esta tarjeta no desbloquea la sesión.",
        }
    }
    pub fn system_operation(self, operation: Operation) -> &'static str {
        match (self, operation) {
            (Self::Ru, Operation::Logout) => "Выйти из сеанса",
            (Self::Ru, Operation::Reboot) => "Перезагрузить",
            (Self::Ru, Operation::PowerOff) => "Выключить",
            (Self::Es, Operation::Logout) => "Cerrar sesión",
            (Self::Es, Operation::Reboot) => "Reiniciar",
            (Self::Es, Operation::PowerOff) => "Apagar",
            (_, Operation::Logout) => "Log out",
            (_, Operation::Reboot) => "Restart",
            (_, Operation::PowerOff) => "Power off",
        }
    }
    pub fn system_capability(self, capability: Capability) -> &'static str {
        match (self, capability) {
            (Self::Ru, Capability::Checking) => "Проверка доступности…",
            (Self::Ru, Capability::Allowed) => "Доступно",
            (Self::Ru, Capability::Authentication) => "Требуется подтверждение прав",
            (Self::Ru, Capability::Denied) => "Нет разрешения",
            (Self::Ru, Capability::Unsupported) => "Не поддерживается",
            (Self::Ru, Capability::Blocked) => "Заблокировано системой",
            (Self::Ru, Capability::Unavailable) => "Служба недоступна",
            (Self::Es, Capability::Checking) => "Comprobando disponibilidad…",
            (Self::Es, Capability::Allowed) => "Disponible",
            (Self::Es, Capability::Authentication) => "Requiere autenticación",
            (Self::Es, Capability::Denied) => "Sin permiso",
            (Self::Es, Capability::Unsupported) => "No admitido",
            (Self::Es, Capability::Blocked) => "Bloqueado por el sistema",
            (Self::Es, Capability::Unavailable) => "Servicio no disponible",
            (_, Capability::Checking) => "Checking availability…",
            (_, Capability::Allowed) => "Available",
            (_, Capability::Authentication) => "Authentication required",
            (_, Capability::Denied) => "Not permitted",
            (_, Capability::Unsupported) => "Not supported",
            (_, Capability::Blocked) => "Blocked by the system",
            (_, Capability::Unavailable) => "Service unavailable",
        }
    }
    pub fn system_confirm(self, operation: Operation) -> &'static str {
        match (self, operation) {
            (Self::Ru, Operation::Logout) => "Выйти из сеанса?",
            (Self::Ru, Operation::Reboot) => "Перезагрузить устройство?",
            (Self::Ru, Operation::PowerOff) => "Выключить устройство?",
            (Self::Es, Operation::Logout) => "¿Cerrar sesión?",
            (Self::Es, Operation::Reboot) => "¿Reiniciar el dispositivo?",
            (Self::Es, Operation::PowerOff) => "¿Apagar el dispositivo?",
            (_, Operation::Logout) => "Log out?",
            (_, Operation::Reboot) => "Restart the device?",
            (_, Operation::PowerOff) => "Power off the device?",
        }
    }
    pub fn system_refresh_hint(self) -> &'static str {
        match self {
            Self::Ru => "Повтор проверит доступность снова.",
            Self::En => "Retry checks availability again.",
            Self::Es => "Reint. vuelve a comprobar la disponibilidad.",
        }
    }
    pub fn system_warning(self, operation: Operation) -> &'static str {
        match (self, operation) {
            (Self::Ru, Operation::Logout) => "Текущий сеанс завершится. Приложения могут закрыться, несохранённые данные могут быть потеряны.",
            (Self::Ru, Operation::Reboot) => "Устройство перезагрузится. Приложения могут закрыться, несохранённые данные могут быть потеряны.",
            (Self::Ru, Operation::PowerOff) => "Устройство выключится. Приложения могут закрыться, несохранённые данные могут быть потеряны.",
            (Self::Es, Operation::Logout) => "Se cerrará la sesión actual. Las aplicaciones pueden cerrarse y se pueden perder los cambios sin guardar.",
            (Self::Es, Operation::Reboot) => "Se reiniciará el dispositivo. Las aplicaciones pueden cerrarse y se pueden perder los cambios sin guardar.",
            (Self::Es, Operation::PowerOff) => "Se apagará el dispositivo. Las aplicaciones pueden cerrarse y se pueden perder los cambios sin guardar.",
            (_, Operation::Logout) => "The current session will end. Applications may close and unsaved changes may be lost.",
            (_, Operation::Reboot) => "The device will restart. Applications may close and unsaved changes may be lost.",
            (_, Operation::PowerOff) => "The device will power off. Applications may close and unsaved changes may be lost.",
        }
    }
    pub fn system_waiting(self) -> &'static str {
        match self {
            Self::Ru => "Ожидание ответа системы. Если требуется пароль, появится отдельное окно подтверждения прав.",
            Self::En => "Waiting for the system. If a password is required, a separate authentication window will appear.",
            Self::Es => "Esperando la respuesta del sistema. Si se necesita una contraseña, aparecerá una ventana de autenticación.",
        }
    }
    pub fn system_accepted(self) -> &'static str {
        match self {
            Self::Ru => "Запрос принят системой. Завершение действия ещё не подтверждено.",
            Self::En => "The system accepted the request. Completion has not yet been confirmed.",
            Self::Es => "El sistema aceptó la solicitud. Aún no se ha confirmado que la acción haya terminado.",
        }
    }
    pub fn system_failure(self, failure: Failure) -> &'static str {
        match (self, failure) {
            (Self::Ru, Failure::Unavailable) => "Служба системных действий недоступна. Попробуйте позже.",
            (Self::Ru, Failure::Denied) => "Система не разрешила это действие.",
            (Self::Ru, Failure::Cancelled) => "Подтверждение прав отменено.",
            (Self::Ru, Failure::Blocked) => "Действие заблокировано системой или другой программой. Завершите текущую работу и попробуйте позже.",
            (Self::Ru, Failure::SessionUnavailable) => "Не удалось определить текущий графический сеанс. Выход недоступен.",
            (Self::Ru, Failure::Timeout) => "Служба не ответила вовремя. Действие не отправлено.",
            (Self::Ru, Failure::Unknown) => "Результат запроса неизвестен.",
            (Self::Es, Failure::Unavailable) => "El servicio de acciones del sistema no está disponible. Inténtelo más tarde.",
            (Self::Es, Failure::Denied) => "El sistema no permitió esta acción.",
            (Self::Es, Failure::Cancelled) => "Se canceló la autenticación.",
            (Self::Es, Failure::Blocked) => "El sistema u otra aplicación bloquea la acción. Termine el trabajo en curso e inténtelo más tarde.",
            (Self::Es, Failure::SessionUnavailable) => "No se pudo identificar la sesión gráfica actual. No se puede cerrar la sesión.",
            (Self::Es, Failure::Timeout) => "El servicio no respondió a tiempo. No se envió la acción.",
            (Self::Es, Failure::Unknown) => "Se desconoce el resultado de la solicitud.",
            (_, Failure::Unavailable) => "The system action service is unavailable. Try again later.",
            (_, Failure::Denied) => "The system did not permit this action.",
            (_, Failure::Cancelled) => "Authentication was cancelled.",
            (_, Failure::Blocked) => "The system or another application is blocking this action. Finish the work in progress and try again later.",
            (_, Failure::SessionUnavailable) => "The current graphical session could not be identified. Log out is unavailable.",
            (_, Failure::Timeout) => "The service did not respond in time. The action was not sent.",
            (_, Failure::Unknown) => "The result of the request is unknown.",
        }
    }
    pub fn search_title(self) -> &'static str {
        match self {
            Self::Ru => "Поиск приложений",
            Self::En => "Search apps",
            Self::Es => "Buscar aplicaciones",
        }
    }
    pub fn search_hint(self) -> &'static str {
        match self {
            Self::Ru => "Имя или ID приложения",
            Self::En => "Name or app ID",
            Self::Es => "Nombre o ID de la app",
        }
    }
    pub fn search_empty(self) -> &'static str {
        match self {
            Self::Ru => "Приложения не найдены",
            Self::En => "No matching apps",
            Self::Es => "No hay coincidencias",
        }
    }
    pub fn search_matches(self, count: usize) -> String {
        match self {
            Self::Ru => format!("Найдено: {count}"),
            Self::En => format!("Matches: {count}"),
            Self::Es => format!("Resultados: {count}"),
        }
    }
    pub fn search_clear_hint(self) -> &'static str {
        match self {
            Self::Ru => "Ctrl+U: очистить",
            Self::En => "Ctrl+U: clear",
            Self::Es => "Ctrl+U: borrar",
        }
    }
    pub fn search_apply_key(self) -> &'static str {
        match self {
            Self::Ru => "Enter: Готово",
            Self::En => "Enter: Apply",
            Self::Es => "Enter: Aplicar",
        }
    }
    pub fn search_cancel_key(self) -> &'static str {
        match self {
            Self::Ru => "Esc: Отмена",
            Self::En => "Esc: Cancel",
            Self::Es => "Esc: Anular",
        }
    }
    pub fn notification_title(self) -> &'static str {
        match self {
            Self::Ru => "Уведомление",
            Self::En => "Notification",
            Self::Es => "Aviso",
        }
    }
    pub fn notification_source(self) -> &'static str {
        match self {
            Self::Ru => "От",
            Self::En => "From",
            Self::Es => "De",
        }
    }
    pub fn unknown_application(self) -> &'static str {
        match self {
            Self::Ru => "Неизвестное приложение",
            Self::En => "Unknown application",
            Self::Es => "Aplicación desconocida",
        }
    }
    pub fn notification_expired(self) -> &'static str {
        match self {
            Self::Ru => "Закрыто; действия недоступны",
            Self::En => "Closed; actions unavailable",
            Self::Es => "Cerrado; acciones no disponibles",
        }
    }
    pub fn notification_unavailable(self) -> &'static str {
        match self {
            Self::Ru => "Служба уведомлений недоступна",
            Self::En => "Notification service unavailable",
            Self::Es => "Servicio de avisos no disponible",
        }
    }
    pub fn notification_actions(self) -> &'static str {
        match self {
            Self::Ru => "Действия уведомления",
            Self::En => "Notification actions",
            Self::Es => "Acciones del aviso",
        }
    }
    pub fn application_option(self, option: &ApplicationOption) -> &'static str {
        match (self, option) {
            (Self::Ru, ApplicationOption::Close) => "Закрыть окно",
            (Self::Es, ApplicationOption::Close) => "Cerrar ventana",
            (_, ApplicationOption::Close) => "Close window",
            (_, ApplicationOption::Windows) => self.application_windows(),
            (Self::Ru, ApplicationOption::Retry { .. }) => "Повторить запуск",
            (Self::Es, ApplicationOption::Retry { .. }) => "Reintentar inicio",
            (_, ApplicationOption::Retry { .. }) => "Retry launch",
            (_, ApplicationOption::Details) => self.application_details(),
        }
    }
    pub fn notification_option(self, option: &NotificationOption) -> &str {
        match (self, option) {
            (_, NotificationOption::Invoke { key, label }) => {
                if !label.is_empty() {
                    label
                } else if key == "default" {
                    self.action(Action::Open)
                } else {
                    key
                }
            }
            (Self::Ru, NotificationOption::Delete) => "Удалить уведомление",
            (Self::Es, NotificationOption::Delete) => "Borrar aviso",
            (_, NotificationOption::Delete) => "Delete notification",
            (Self::Ru, NotificationOption::Clear) => "Очистить все уведомления",
            (Self::Es, NotificationOption::Clear) => "Borrar todos los avisos",
            (_, NotificationOption::Clear) => "Clear all notifications",
            (Self::Ru, NotificationOption::Retry) => "Повторить подключение к службе",
            (Self::Es, NotificationOption::Retry) => "Reconectar con el servicio",
            (_, NotificationOption::Retry) => "Reconnect to service",
        }
    }
    pub fn application_windows(self) -> &'static str {
        match self {
            Self::Ru => "Окна",
            Self::Es => "Ventanas",
            Self::En => "Windows",
        }
    }
    pub fn application_details(self) -> &'static str {
        match self {
            Self::Ru => "Сведения",
            Self::Es => "Detalles",
            Self::En => "Details",
        }
    }
    pub fn window_identifier(self) -> &'static str {
        match self {
            Self::Ru => "Окно",
            Self::Es => "Ventana",
            Self::En => "Window",
        }
    }
    pub fn launch_attempt(self) -> &'static str {
        match self {
            Self::Ru => "Попытка",
            Self::Es => "Intento",
            Self::En => "Attempt",
        }
    }
    pub fn launch_phase(self, phase: LaunchPhase) -> &'static str {
        match (self, phase) {
            (Self::Ru, LaunchPhase::Starting) => "Запускается…",
            (Self::Ru, LaunchPhase::Unconfirmed) => "Окно не обнаружено",
            (Self::Ru, LaunchPhase::Failed) => "Ошибка запуска",
            (Self::Es, LaunchPhase::Starting) => "Iniciando…",
            (Self::Es, LaunchPhase::Unconfirmed) => "No se detectó una ventana",
            (Self::Es, LaunchPhase::Failed) => "Error de inicio",
            (_, LaunchPhase::Starting) => "Starting…",
            (_, LaunchPhase::Unconfirmed) => "Window not detected",
            (_, LaunchPhase::Failed) => "Launch failed",
        }
    }
    pub fn window_operation(self, operation: WindowOperation) -> &'static str {
        match (self, operation) {
            (Self::Ru, WindowOperation::Activating) => "Переход к окну…",
            (Self::Ru, WindowOperation::Closing) => "Закрывается…",
            (Self::Ru, WindowOperation::ActivationUnconfirmed) => "Переход не подтверждён",
            (Self::Ru, WindowOperation::CloseUnconfirmed) => "Закрытие не подтверждено",
            (Self::Es, WindowOperation::Activating) => "Activando ventana…",
            (Self::Es, WindowOperation::Closing) => "Cerrando…",
            (Self::Es, WindowOperation::ActivationUnconfirmed) => "Activación no confirmada",
            (Self::Es, WindowOperation::CloseUnconfirmed) => "Cierre no confirmado",
            (_, WindowOperation::Activating) => "Activating window…",
            (_, WindowOperation::Closing) => "Closing…",
            (_, WindowOperation::ActivationUnconfirmed) => "Activation unconfirmed",
            (_, WindowOperation::CloseUnconfirmed) => "Close unconfirmed",
        }
    }
    pub fn history_explanation(self) -> &'static str {
        match self {
            Self::Ru => {
                "Запуск открывает приложение. Содержимое закрытого окна не восстанавливается."
            }
            Self::Es => {
                "El inicio abre la aplicación. No restaura el contenido de la ventana cerrada."
            }
            Self::En => {
                "Launch opens the application. It does not restore the closed window's contents."
            }
        }
    }
    pub fn confirm_retry(self) -> &'static str {
        match self {
            Self::Ru => "Повторить запуск?",
            Self::Es => "¿Reintentar inicio?",
            Self::En => "Retry launch?",
        }
    }
    pub fn retry_explanation(self) -> &'static str {
        match self {
            Self::Ru => "Процесс может ещё работать. Повтор может открыть вторую копию.",
            Self::Es => "El proceso puede seguir activo. Reintentar puede abrir otra copia.",
            Self::En => "The process may still be running. Retrying may open a second copy.",
        }
    }
    pub fn link(self, state: LinkState) -> &'static str {
        match (self, state) {
            (Self::Ru, LinkState::Unknown) => "нет данных",
            (Self::Ru, LinkState::Absent) => "нет устройства",
            (Self::Ru, LinkState::Disabled) => "выключено",
            (Self::Ru, LinkState::Disconnected) => "не подключено",
            (Self::Ru, LinkState::Connecting) => "подключение",
            (Self::Ru, LinkState::LinkOnly) => "без IP-адреса",
            (Self::Ru, LinkState::Addressed) => "только локально",
            (Self::Ru, LinkState::Routed) => "подключено",
            (Self::Es, LinkState::Unknown) => "sin datos",
            (Self::Es, LinkState::Absent) => "sin dispositivo",
            (Self::Es, LinkState::Disabled) => "apagado",
            (Self::Es, LinkState::Disconnected) => "sin conexión",
            (Self::Es, LinkState::Connecting) => "conectando",
            (Self::Es, LinkState::LinkOnly) => "sin dirección IP",
            (Self::Es, LinkState::Addressed) => "solo red local",
            (Self::Es, LinkState::Routed) => "conectado",
            (_, LinkState::Unknown) => "unknown",
            (_, LinkState::Absent) => "not present",
            (_, LinkState::Disabled) => "off",
            (_, LinkState::Disconnected) => "disconnected",
            (_, LinkState::Connecting) => "connecting",
            (_, LinkState::LinkOnly) => "no IP address",
            (_, LinkState::Addressed) => "local network only",
            (_, LinkState::Routed) => "connected",
        }
    }
    pub fn battery(self, state: BatteryState) -> String {
        let label = match self {
            Self::Ru => "Батарея",
            Self::En => "Battery",
            Self::Es => "Batería",
        };
        let content = match state {
            BatteryState::Unknown => self.link(LinkState::Unknown).to_string(),
            BatteryState::Absent => self.link(LinkState::Absent).to_string(),
            BatteryState::Present { percent, charge } => {
                let percent = percent.map_or_else(|| "?".to_string(), |n| format!("{n}%"));
                let charge = match (self, charge) {
                    (Self::Ru, ChargeState::Unknown) => "статус неизвестен",
                    (Self::Ru, ChargeState::Charging) => "зарядка",
                    (Self::Ru, ChargeState::Discharging) => "разряд",
                    (Self::Ru, ChargeState::Full) => "заряжена",
                    (Self::Ru, ChargeState::NotCharging) => "не заряжается",
                    (Self::Es, ChargeState::Unknown) => "estado desconocido",
                    (Self::Es, ChargeState::Charging) => "cargando",
                    (Self::Es, ChargeState::Discharging) => "descargando",
                    (Self::Es, ChargeState::Full) => "llena",
                    (Self::Es, ChargeState::NotCharging) => "sin cargar",
                    (_, ChargeState::Unknown) => "state unknown",
                    (_, ChargeState::Charging) => "charging",
                    (_, ChargeState::Discharging) => "discharging",
                    (_, ChargeState::Full) => "full",
                    (_, ChargeState::NotCharging) => "not charging",
                };
                format!("{percent} {charge}")
            }
        };
        format!("{label}: {content}")
    }
    pub fn running(self) -> &'static str {
        match self {
            Self::Ru => "Работает",
            Self::Es => "Activo",
            Self::En => "Running",
        }
    }
    pub fn closed(self) -> &'static str {
        match self {
            Self::Ru => "Закрыто",
            Self::Es => "Cerrado",
            Self::En => "Closed",
        }
    }
    pub fn empty(self) -> &'static str {
        match self {
            Self::Ru => "Пока пусто",
            Self::Es => "Sin elementos",
            Self::En => "Nothing here yet",
        }
    }
    pub fn window_manager_unavailable(self) -> &'static str {
        match self {
            Self::Ru => "Нет доступа к окнам",
            Self::Es => "Ventanas no disponibles",
            Self::En => "Window list unavailable",
        }
    }
    pub fn info(self, info: InfoKind) -> &'static str {
        match (self, info) {
            (Self::Ru, InfoKind::Unsupported) => "Функция ещё не доступна.",
            (Self::Ru, InfoKind::Activation) => "Не удалось перейти к окну.",
            (Self::Ru, InfoKind::Close) => "Не удалось закрыть окно.",
            (Self::Ru, InfoKind::LaunchError) => "Не удалось запустить приложение.",
            (Self::Ru, InfoKind::NotificationError) => {
                "Уведомление изменилось или действие недоступно."
            }
            (Self::Ru, InfoKind::TargetUnavailable) => "Окно уже недоступно",
            (Self::Es, InfoKind::Unsupported) => "Función no disponible todavía.",
            (Self::Es, InfoKind::Activation) => "No se pudo activar la ventana.",
            (Self::Es, InfoKind::Close) => "No se pudo cerrar la ventana.",
            (Self::Es, InfoKind::LaunchError) => "No se pudo abrir la aplicación.",
            (Self::Es, InfoKind::NotificationError) => {
                "El aviso cambió o la acción no está disponible."
            }
            (Self::Es, InfoKind::TargetUnavailable) => "La ventana ya no está disponible",
            (_, InfoKind::Unsupported) => "Feature not available yet.",
            (_, InfoKind::Activation) => "Could not activate the window.",
            (_, InfoKind::Close) => "Could not close the window.",
            (_, InfoKind::LaunchError) => "Failed to launch application.",
            (_, InfoKind::NotificationError) => {
                "The notification changed or its action is unavailable."
            }
            (_, InfoKind::TargetUnavailable) => "The window is no longer available",
        }
    }
    pub fn confirm_close(self) -> &'static str {
        match self {
            Self::Ru => "Закрыть окно?",
            Self::Es => "¿Cerrar ventana?",
            Self::En => "Close window?",
        }
    }
    pub fn confirm_clear(self) -> &'static str {
        match self {
            Self::Ru => "Очистить уведомления?",
            Self::Es => "¿Borrar los avisos?",
            Self::En => "Clear notifications?",
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/i18n.rs"]
mod tests;
